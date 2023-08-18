// Copyright (c) 2023 the Hearth contributors.
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// This file is part of Hearth.
//
// Hearth is free software: you can redistribute it and/or modify it under the
// terms of the GNU Affero General Public License as published by the Free
// Software Foundation, either version 3 of the License, or (at your option)
// any later version.
//
// Hearth is distributed in the hope that it will be useful, but WITHOUT ANY
// WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more
// details.
//
// You should have received a copy of the GNU Affero General Public License
// along with Hearth. If not, see <https://www.gnu.org/licenses/>.

//! Hearth runtime construction and the plugin interface.
//!
//! To get started, call [RuntimeBuilder::new] to start building a runtime,
//! then add plugins, runners, or asset loaders to the builder. When finished,
//! call [RuntimeBuilder::run] to start up the configured runtime.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use hearth_types::Flags;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, warn};

use crate::asset::{AssetLoader, AssetStore};
use crate::lump::LumpStoreImpl;
use crate::process::factory::ProcessInfo;

/// Interface trait for plugins to the Hearth runtime.
///
/// Each plugin first builds onto a runtime using its `build` function and an
/// in-progress [RuntimeBuilder]. During this phase, plugins can mutably access
/// other plugins that have already been added. When all the plugins have been
/// added, the final phase of runtime building begins. Each plugin's `run`
/// method takes ownership of the plugin and finishes adding onto the
/// [RuntimeBuilder] using the complete configuration for that plugin.
#[async_trait]
pub trait Plugin: Sized + Send + Sync + 'static {
    /// Builds a runtime using this plugin. See [RuntimeBuilder] for more info.
    fn build(&mut self, _builder: &mut RuntimeBuilder) {}

    /// Finishes building this runtime before the runtime starts. See [RuntimeBuilder] for more info.
    fn finish(self, _builder: &mut RuntimeBuilder) {}
}

struct PluginWrapper {
    plugin: Box<dyn Any + Send>,
    finish: Box<dyn FnOnce(Box<dyn Any>, &mut RuntimeBuilder) + Send>,
}

/// Builder struct for a single Hearth [Runtime].
pub struct RuntimeBuilder {
    config_file: toml::Table,
    plugins: HashMap<TypeId, PluginWrapper>,
    runners: Vec<Box<dyn FnOnce(Arc<Runtime>) + Send>>,
    services: HashSet<String>,
    lump_store: Arc<LumpStoreImpl>,
    asset_store: AssetStore,
    service_num: usize,
    service_start_tx: UnboundedSender<String>,
    service_start_rx: UnboundedReceiver<String>,
}

impl RuntimeBuilder {
    /// Creates a new [RuntimeBuilder] with nothing loaded.
    pub fn new(config_file: toml::Table) -> Self {
        let lump_store = Arc::new(LumpStoreImpl::new());
        let asset_store = AssetStore::new(lump_store.clone());
        let (service_start_tx, service_start_rx) = unbounded_channel();

        Self {
            config_file,
            plugins: Default::default(),
            runners: Default::default(),
            services: Default::default(),
            lump_store,
            asset_store,
            service_num: 0,
            service_start_tx,
            service_start_rx,
        }
    }

    /// Loads a configuration value from a table in the config file.
    pub fn load_config<T: serde::de::DeserializeOwned>(&self, table: &str) -> anyhow::Result<T> {
        let value = self
            .config_file
            .get(table)
            .ok_or_else(|| anyhow::anyhow!("No table '{}' in config file", table))?
            .to_owned();

        T::deserialize(value).map_err(|err| {
            anyhow::anyhow!("Failed to deserialize '{}' in config: {:?}", table, err)
        })
    }

    /// Adds a plugin to the runtime.
    ///
    /// Plugins may use their [Plugin::build] method to add other plugins,
    /// asset loaders, runners, or anything else. Then, plugins may configure
    /// already-added plugins using [RuntimeBuilder::get_plugin] and
    /// [RuntimeBuilder::get_plugin_mut]. After all plugins have been added
    /// and before the runtime is started, [Plugin::finish] is called with
    /// each plugin to complete the plugin's building.
    pub fn add_plugin<T: Plugin>(&mut self, mut plugin: T) -> &mut Self {
        let name = std::any::type_name::<T>();
        debug!("Adding {} plugin", name);

        let id = plugin.type_id();
        if self.plugins.contains_key(&id) {
            warn!("Attempted to add {} plugin twice", name);
            return self;
        }

        plugin.build(self);

        self.plugins.insert(
            id,
            PluginWrapper {
                plugin: Box::new(plugin),
                finish: Box::new(move |plugin, builder| {
                    let plugin = plugin.downcast::<T>().unwrap();
                    debug!("Finishing {} plugin", name);
                    plugin.finish(builder);
                }),
            },
        );

        self
    }

    /// Adds a runner to the runtime.
    ///
    /// Runners are functions that are spawned when the runtime is started and
    /// are passed a handle to the new runtime. This may be used to spawn tasks
    /// to handle long-running event processing code or other functionality
    /// that lasts the runtime's lifetime.
    pub fn add_runner<F>(&mut self, cb: F) -> &mut Self
    where
        F: FnOnce(Arc<Runtime>) + Send + 'static,
    {
        self.runners.push(Box::new(cb));
        self
    }

    /// Adds a service.
    ///
    /// Logs a warning if the new service replaces another one.
    ///
    /// Behind the scenes this creates a runner that spawns the process and
    /// registers it as a service.
    pub fn add_service(
        &mut self,
        name: String,
        info: ProcessInfo,
        flags: Flags,
        cb: impl FnOnce(Arc<Runtime>, crate::process::Process) + Send + 'static,
    ) -> &mut Self {
        if self.services.contains(&name) {
            error!("Service name {} is taken", name);
            return self;
        }

        let service_start_tx = self.service_start_tx.clone();
        self.service_num += 1;

        self.services.insert(name.clone());
        self.add_runner(move |runtime| {
            tokio::spawn(async move {
                debug!("Spawning '{}' service", name);
                let process = runtime.process_factory.spawn(info, flags);
                let self_cap = process
                    .get_cap(0)
                    .expect("freshly-spawned process has no self cap")
                    .clone(runtime.process_store.as_ref());
                if let Some(old_cap) = runtime.process_registry.insert(name.clone(), self_cap) {
                    warn!("Service name {:?} was taken; replacing", name);
                    old_cap.free(runtime.process_store.as_ref());
                }

                let _ = service_start_tx.send(name);

                cb(runtime, process);
            });
        });

        self
    }

    /// Adds a new asset loader.
    ///
    /// Logs an error event if the asset loader has already been added.
    pub fn add_asset_loader(&mut self, loader: impl AssetLoader) -> &mut Self {
        self.asset_store.add_loader(loader);
        self
    }

    /// Retrieves a reference to a plugin that has already been added.
    ///
    /// This function is intended to be used for dependencies of plugins, where
    /// a plugin may need to look up or modify the contents of a previously-
    /// added plugin. Using this function saves the code building the runtime
    /// the trouble of manually passing runtimes to other runtimes as
    /// dependencies.
    pub fn get_plugin<T: Plugin>(&self) -> Option<&T> {
        self.plugins
            .get(&TypeId::of::<T>())
            .and_then(|p| p.plugin.downcast_ref())
    }

    /// Retrieves a mutable reference to a plugin that has already been added.
    ///
    /// Mutable version of [Self::get_plugin].
    pub fn get_plugin_mut<T: Plugin>(&mut self) -> Option<&mut T> {
        self.plugins
            .get_mut(&TypeId::of::<T>())
            .and_then(|p| p.plugin.downcast_mut())
    }

    /// Consumes this builder and starts up the full [Runtime].
    ///
    /// This returns a shared pointer to the new runtime.
    pub async fn run(mut self, config: RuntimeConfig) -> Arc<Runtime> {
        debug!("Finishing plugins");
        loop {
            let plugins = std::mem::take(&mut self.plugins);

            if plugins.is_empty() {
                break;
            }

            for (_id, wrapper) in plugins {
                let PluginWrapper { plugin, finish } = wrapper;
                finish(plugin, &mut self);
            }
        }

        use crate::process::*;

        let process_store = Arc::new(ProcessStore::default());
        let process_registry = Arc::new(Registry::new(process_store.clone()));
        let process_factory = Arc::new(ProcessFactory::new(
            process_store.clone(),
            process_registry.clone(),
        ));

        let lump_store = self.lump_store;

        let runtime = Arc::new(Runtime {
            asset_store: Arc::new(self.asset_store),
            lump_store,
            process_store,
            process_registry,
            process_factory,
            config,
        });

        debug!("Running runners");
        for runner in self.runners {
            runner(runtime.clone());
        }

        let service_num = self.service_num;
        let mut service_rx = self.service_start_rx;
        debug!("Waiting for {} services to start...", service_num);
        for i in 0..service_num {
            let name = service_rx.recv().await.expect(
                "all instances of service_start_tx dropped while waiting for all services to start",
            );

            let left = service_num - i;
            debug!("Service {:?} started, {} left", name, left);
        }

        debug!("All services started");

        runtime
    }
}

/// Configuration info for a runtime.
pub struct RuntimeConfig {}

/// An instance of a single Hearth runtime.
///
/// This contains all of the resources that are used by plugins and processes.
/// A runtime can be built and started using [RuntimeBuilder].
///
/// Note that Hearth uses Tokio for all of its asynchronous
/// task execution and IO, so it's assumed that a Tokio runtime has already
/// been created.
pub struct Runtime {
    /// The configuration of this runtime.
    pub config: RuntimeConfig,

    //// The assets in this runtime.
    pub asset_store: Arc<AssetStore>,

    /// This runtime's lump store.
    pub lump_store: Arc<LumpStoreImpl>,

    /// This runtime's process store.
    pub process_store: Arc<crate::process::ProcessStore>,

    /// This runtime's process registry.
    pub process_registry: Arc<crate::process::Registry>,

    /// This runtime's process factory.
    pub process_factory: Arc<crate::process::ProcessFactory>,
}
