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

use std::{
    ops::{Deref, DerefMut},
    path::PathBuf,
    sync::Arc,
};

use hearth_core::{
    process::{Connection, Process},
    runtime::{Plugin, Runtime, RuntimeBuilder},
    tokio::{
        self,
        net::{UnixListener, UnixStream},
        sync::oneshot,
    },
};
use hearth_init::InitPlugin;
use hearth_ipc::get_socket_path;

pub struct Listener {
    pub uds: UnixListener,
    pub path: PathBuf,
}

impl Drop for Listener {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.path) {
            Ok(_) => {}
            Err(e) => tracing::error!("Could not delete UnixListener {:?}", e),
        }
    }
}

impl Deref for Listener {
    type Target = UnixListener;

    fn deref(&self) -> &UnixListener {
        &self.uds
    }
}

impl DerefMut for Listener {
    fn deref_mut(&mut self) -> &mut UnixListener {
        &mut self.uds
    }
}

impl Listener {
    pub async fn new() -> std::io::Result<Self> {
        use std::io::{Error, ErrorKind};

        let sock_path = match get_socket_path() {
            Some(p) => p,
            None => {
                let kind = ErrorKind::NotFound;
                let msg = "Failed to find a socket path";
                tracing::error!(msg);
                return Err(Error::new(kind, msg));
            }
        };

        match UnixStream::connect(&sock_path).await {
            Ok(_) => {
                tracing::warn!(
                    "Socket is already in use. Another instance of Hearth may be running."
                );
                let kind = ErrorKind::AddrInUse;
                let error = Error::new(kind, "Socket is already in use.");
                return Err(error);
            }
            Err(ref err) if err.kind() == ErrorKind::ConnectionRefused => {
                tracing::warn!("Found leftover socket; removing.");
                std::fs::remove_file(&sock_path)?;
            }
            Err(ref err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }

        tracing::info!("Making socket at: {:?}", sock_path);
        let uds = UnixListener::bind(&sock_path)?;
        let path = sock_path.to_path_buf();
        Ok(Self { uds, path })
    }

    pub async fn accept_next(&self) -> hearth_ipc::Connection {
        let stream = loop {
            match self.accept().await {
                Ok((socket, addr)) => {
                    tracing::debug!("Accepting IPC connection from {:?}", addr);
                    break socket;
                }
                Err(err) => {
                    tracing::error!("IPC listen error: {:?}", err);
                }
            }
        };

        let (rx, tx) = stream.into_split();
        hearth_ipc::Connection::new(rx, tx)
    }
}

pub struct DaemonPlugin {}

impl Plugin for DaemonPlugin {
    fn finish(mut self, builder: &mut RuntimeBuilder) {
        let init = builder
            .get_plugin_mut::<InitPlugin>()
            .expect("InitPlugin not found");

        let (root_tx, root_rx) = oneshot::channel();
        init.add_hook("hearth.init.Daemon".into(), root_tx);

        builder.add_runner(move |runtime| {
            tokio::spawn(async move {
                let (root_ctx, root_handle) = match root_rx.await {
                    Ok(root) => root,
                    Err(err) => {
                        tracing::warn!("error while waiting for daemon root cap: {:?}", err);
                        return;
                    }
                };

                let listener = Listener::new().await.unwrap();

                loop {
                    let transport = listener.accept_next().await;
                    self.on_accept(&root_ctx, root_handle, &runtime, transport);
                }
            });
        });
    }
}

impl DaemonPlugin {
    pub fn new() -> Self {
        Self {}
    }

    pub fn on_accept(
        &mut self,
        root_ctx: &Process,
        root_handle: usize,
        runtime: &Arc<Runtime>,
        transport: hearth_ipc::Connection,
    ) {
        tracing::info!("Beginning IPC connection");
        let conn = Connection::new(
            runtime.process_store.clone(),
            transport.op_rx,
            transport.op_tx,
            None,
        );

        tracing::info!("Sending the IPC client our root cap");
        let mut conn = conn.lock();
        let result = root_ctx.export_connection_root(root_handle, &mut conn);
        if let Err(err) = result {
            tracing::error!("error while sending IPC client root cap: {:?}", err);
        }
    }
}
