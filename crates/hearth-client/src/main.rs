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
    net::{SocketAddr, ToSocketAddrs},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};

use clap::Parser;
use hearth_core::{
    flue::OwnedCapability,
    runtime::{Runtime, RuntimeBuilder, RuntimeConfig},
};
use hearth_network::{auth::login, connection::Connection};
use hearth_rend3::Rend3Plugin;
use tokio::{net::TcpStream, sync::oneshot};
use tracing::{debug, error, info};

mod window;

/// Client program to the Hearth virtual space server.
#[derive(Parser, Debug)]
pub struct Args {
    /// IP address and port of the server to connect to.
    // TODO support DNS resolution too
    #[clap(short, long)]
    pub server: String,

    /// Password to use to authenticate to the server. Defaults to empty.
    #[clap(short, long, default_value = "")]
    pub password: String,

    /// A configuration file to use if not the default one.
    #[clap(short, long)]
    pub config: Option<PathBuf>,

    /// The init system to run.
    #[clap(short, long)]
    pub init: PathBuf,

    /// A path to the guest-side filesystem root.
    #[clap(short, long)]
    pub root: PathBuf,
}

fn main() {
    let args = Args::parse();
    hearth_core::init_logging();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let (window_tx, window_rx) = tokio::sync::oneshot::channel();
    let window = window::WindowCtx::new(&runtime, window_tx);

    runtime.block_on(async {
        let mut window = window_rx.await.unwrap();
        let mut join_main = runtime.spawn(async move {
            async_main(args, window.rend3_plugin).await;
        });

        runtime.spawn(async move {
            loop {
                tokio::select! {
                    event = window.event_tx.recv() => {
                        debug!("window event: {:?}", event);
                        if let Some(window::WindowTxMessage::Quit) = event {
                            break;
                        }
                    }
                    _ = &mut join_main => {
                        debug!("async_main joined");
                        window.event_rx.send_event(window::WindowRxMessage::Quit).unwrap();
                        break;
                    }
                }
            }
        });
    });

    debug!("Running window event loop");
    window.run();
}

async fn async_main(args: Args, rend3_plugin: Rend3Plugin) {
    let config = RuntimeConfig {};

    let config_path = args.config.unwrap_or_else(hearth_core::get_config_path);
    let config_file = hearth_core::load_config(&config_path).unwrap();

    let (network_root_tx, network_root_rx) = oneshot::channel();
    let mut init = hearth_init::InitPlugin::new(args.init);
    init.add_hook("hearth.init.Client".into(), network_root_tx);

    let mut builder = RuntimeBuilder::new(config_file);
    builder.add_plugin(hearth_cognito::WasmPlugin::default());
    builder.add_plugin(hearth_fs::FsPlugin::new(args.root));
    builder.add_plugin(rend3_plugin);
    builder.add_plugin(hearth_terminal::TerminalPlugin::default());
    builder.add_plugin(init);
    builder.add_plugin(hearth_daemon::DaemonPlugin::default());
    let runtime = builder.run(config).await;

    tokio::spawn(async move {
        connect(network_root_rx, runtime, args.server, args.password).await;
    });

    hearth_core::wait_for_interrupt().await;
    info!("Ctrl+C hit; quitting client");
}

async fn connect(
    on_network_root: oneshot::Receiver<OwnedCapability>,
    runtime: Arc<Runtime>,
    server: String,
    password: String,
) {
    info!("Waiting for network root cap hook");
    let network_root = on_network_root.await.unwrap();

    info!("Resolving {}", server);
    let server = match SocketAddr::from_str(&server) {
        Err(_) => {
            info!(
                "Failed to parse \'{}\' to SocketAddr, attempting DNS resolution",
                server
            );
            match server.to_socket_addrs() {
                Err(err) => {
                    error!("Failed to resolve IP: {:?}", err);
                    return;
                }
                Ok(addrs) => match addrs.last() {
                    None => return,
                    Some(addr) => addr,
                },
            }
        }
        Ok(addr) => addr,
    };

    info!("Connecting to server at {:?}", server);
    let mut socket = match TcpStream::connect(server).await {
        Ok(s) => s,
        Err(err) => {
            error!("Failed to connect to server: {:?}", err);
            return;
        }
    };

    info!("Authenticating");
    let session_key = match login(&mut socket, password.as_bytes()).await {
        Ok(key) => key,
        Err(err) => {
            error!("Failed to authenticate with server: {:?}", err);
            return;
        }
    };

    use hearth_network::encryption::{AsyncDecryptor, AsyncEncryptor, Key};
    let client_key = Key::from_client_session(&session_key);
    let server_key = Key::from_server_session(&session_key);

    let (server_rx, server_tx) = tokio::io::split(socket);
    let server_rx = AsyncDecryptor::new(&server_key, server_rx);
    let server_tx = AsyncEncryptor::new(&client_key, server_tx);
    let conn = Connection::new(server_rx, server_tx);

    info!("Beginning connection");
    let (root_cap_tx, root_cap) = tokio::sync::oneshot::channel();
    let conn = hearth_core::connection::Connection::begin(
        runtime.post.clone(),
        conn.op_rx,
        conn.op_tx,
        Some(root_cap_tx),
    );

    info!("Sending the server our root cap");
    conn.export_root(network_root);

    info!("Waiting for server's root cap...");
    let _root_cap = match root_cap.await {
        Ok(cap) => cap,
        Err(err) => {
            eprintln!("Server's root cap was never received: {:?}", err);
            return;
        }
    };

    info!("Successfully connected!");
}
