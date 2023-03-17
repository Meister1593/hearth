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

use std::sync::Arc;

use anyhow::{anyhow, Result};
use hearth_core::lump::{bytes::Bytes, LumpStoreImpl};
use hearth_core::process::{Process, ProcessContext};
use hearth_core::runtime::{Plugin, Runtime, RuntimeBuilder};
use hearth_macros::impl_wasm_linker;
use hearth_rpc::hearth_types::LumpId;
use hearth_rpc::{remoc, ProcessInfo};
use hearth_wasm::{GuestMemory, WasmLinker};
use remoc::rtc::async_trait;
use slab::Slab;
use tracing::{debug, error};
use wasmtime::*;

/// Implements the `hearth::asset` ABI module.
#[derive(Debug, Default)]
pub struct AssetAbi {}

#[impl_wasm_linker(module = "hearth::asset")]
impl AssetAbi {}

/// Implements the `hearth::log` ABI module.
#[derive(Debug, Default)]
pub struct LogAbi {}

#[impl_wasm_linker(module = "hearth::log")]
impl LogAbi {}

/// A script-local lump stored in [LumpAbi].
#[derive(Debug)]
pub struct LocalLump {
    pub id: LumpId,
    pub bytes: Bytes,
}

/// Implements the `hearth::lump` ABI module.
#[derive(Debug, Default)]
pub struct LumpAbi {
    pub lump_store: Arc<LumpStoreImpl>,
    pub lump_handles: Slab<LocalLump>,
}

#[impl_wasm_linker(module = "hearth::lump")]
impl LumpAbi {
    async fn from_id(&mut self, memory: GuestMemory<'_>, id_ptr: u32) -> Result<u32> {
        let id: LumpId = *memory.get_memory_ref(id_ptr)?;
        let bytes = self
            .lump_store
            .get_lump(&id)
            .await
            .ok_or_else(|| anyhow!("Couldn't find {:?} in lump store", id))?;
        Ok(self.lump_handles.insert(LocalLump { id, bytes }) as u32)
    }

    async fn load(&mut self, memory: GuestMemory<'_>, ptr: u32, len: u32) -> Result<u32> {
        let bytes: Bytes = memory
            .get_slice(ptr as usize, len as usize)?
            .to_vec()
            .into();
        let id = self.lump_store.add_lump(bytes.clone()).await;
        let lump = LocalLump { id, bytes };
        let handle = self.lump_handles.insert(lump) as u32;
        Ok(handle)
    }

    fn get_id(&self, memory: GuestMemory<'_>, handle: u32, id_ptr: u32) -> Result<()> {
        let lump = self.get_lump(handle)?;
        let id: &mut LumpId = memory.get_memory_ref(id_ptr)?;
        *id = lump.id;
        Ok(())
    }

    fn get_len(&self, handle: u32) -> Result<u32> {
        self.get_lump(handle).map(|lump| lump.bytes.len() as u32)
    }

    fn get_data(&self, memory: GuestMemory<'_>, handle: u32, ptr: u32) -> Result<()> {
        let lump = self.get_lump(handle)?;
        let len = lump.bytes.len();
        let dst = memory.get_slice(ptr as usize, len)?;
        dst.copy_from_slice(&lump.bytes);
        Ok(())
    }

    fn free(&mut self, handle: u32) -> Result<()> {
        self.lump_handles
            .try_remove(handle as usize)
            .map(|_| ())
            .ok_or_else(|| anyhow!("Lump handle {} is invalid", handle))
    }
}

impl LumpAbi {
    fn get_lump(&self, handle: u32) -> Result<&LocalLump> {
        self.lump_handles
            .get(handle as usize)
            .ok_or_else(|| anyhow!("Lump handle {} is invalid", handle))
    }
}

/// Implements the `hearth::message` ABI module.
#[derive(Debug, Default)]
pub struct MessageAbi {}

#[impl_wasm_linker(module = "hearth::message")]
impl MessageAbi {}

/// Implements the `hearth::process` ABI module.
#[derive(Debug, Default)]
pub struct ProcessAbi {}

#[impl_wasm_linker(module = "hearth::process")]
impl ProcessAbi {}

/// Implements the `hearth::service` ABI module.
#[derive(Debug, Default)]
pub struct ServiceAbi {}

#[impl_wasm_linker(module = "hearth::service")]
impl ServiceAbi {}

/// This contains all script-accessible process-related stuff.
#[derive(Debug, Default)]
pub struct ProcessData {
    pub asset: AssetAbi,
    pub log: LogAbi,
    pub lump: LumpAbi,
    pub message: MessageAbi,
    pub process: ProcessAbi,
    pub service: ServiceAbi,
}

macro_rules! impl_asmut {
    ($ty: ident, $sub_ty: ident, $sub_field: ident) => {
        impl ::std::convert::AsMut<$sub_ty> for $ty {
            fn as_mut(&mut self) -> &mut $sub_ty {
                &mut self.$sub_field
            }
        }
    };
}

impl_asmut!(ProcessData, AssetAbi, asset);
impl_asmut!(ProcessData, LogAbi, log);
impl_asmut!(ProcessData, LumpAbi, lump);
impl_asmut!(ProcessData, MessageAbi, message);
impl_asmut!(ProcessData, ProcessAbi, process);
impl_asmut!(ProcessData, ServiceAbi, service);

impl ProcessData {
    /// Adds all module ABIs to the given linker.
    pub fn add_to_linker(linker: &mut Linker<Self>) {
        AssetAbi::add_to_linker(linker);
        LogAbi::add_to_linker(linker);
        LumpAbi::add_to_linker(linker);
        MessageAbi::add_to_linker(linker);
        ProcessAbi::add_to_linker(linker);
        ServiceAbi::add_to_linker(linker);
    }
}

struct WasmProcess {
    engine: Arc<Engine>,
    linker: Arc<Linker<ProcessData>>,
    module: Arc<Module>,
}

#[async_trait]
impl Process for WasmProcess {
    fn get_info(&self) -> ProcessInfo {
        ProcessInfo {}
    }

    async fn run(&mut self, ctx: ProcessContext) {
        // TODO log using the process log instead of tracing?
        let data = ProcessData::default();
        let mut store = Store::new(&self.engine, data);
        let instance = match self
            .linker
            .instantiate_async(&mut store, &self.module)
            .await
        {
            Ok(instance) => instance,
            Err(err) => {
                error!("Failed to instantiate WasmProcess: {:?}", err);
                return;
            }
        };

        // TODO better wasm invocation?
        match instance.get_typed_func::<(), ()>(&mut store, "run") {
            Ok(run) => {
                if let Err(err) = run.call_async(&mut store, ()).await {
                    error!("Wasm run error: {:?}", err);
                }
            }
            Err(err) => {
                error!("Couldn't find run function: {:?}", err);
            }
        }
    }
}

pub struct WasmProcessSpawner {
    engine: Arc<Engine>,
    linker: Arc<Linker<ProcessData>>,
}

#[async_trait]
impl Process for WasmProcessSpawner {
    fn get_info(&self) -> ProcessInfo {
        ProcessInfo {}
    }

    async fn run(&mut self, mut ctx: ProcessContext) {
        while let Some(message) = ctx.recv().await {
            debug!("WasmProcessSpawner: got message from {:?}", message.sender);
        }
    }
}

impl WasmProcessSpawner {
    pub fn new() -> Self {
        let mut config = Config::new();
        config.async_support(true);

        let engine = Engine::new(&config).unwrap();
        let mut linker = Linker::new(&engine);
        ProcessData::add_to_linker(&mut linker);

        Self {
            engine: Arc::new(engine),
            linker: Arc::new(linker),
        }
    }
}

pub struct WasmPlugin {}

#[async_trait]
impl Plugin for WasmPlugin {
    fn build(&mut self, builder: &mut RuntimeBuilder) {
        let name = "hearth.cognito.WasmProcessSpawner".to_string();
        let spawner = WasmProcessSpawner::new();
        builder.add_service(name, spawner);
    }

    async fn run(&mut self, _runtime: Arc<Runtime>) {
        // WasmProcessSpawner takes care of everything
    }
}

impl WasmPlugin {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link() {
        let mut config = Config::new();
        config.async_support(true);
        let engine = Engine::new(&config).unwrap();
        let mut linker = Linker::new(&engine);
        ProcessData::add_to_linker(&mut linker);
    }
}
