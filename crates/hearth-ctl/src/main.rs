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

use clap::{Parser, Subcommand};
use hearth_rpc::DaemonOffer;

mod list_peers;
mod list_processes;
mod spawn_wasm;

/// Command-line interface (CLI) for interacting with a Hearth daemon over IPC.
#[derive(Debug, Parser)]
pub struct Args {
    #[clap(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    ListPeers(list_peers::ListPeers),
    ListProcesses(list_processes::ListProcesses),
    SpawnWasm(spawn_wasm::SpawnWasm),
}

impl Commands {
    pub async fn run(self, daemon: DaemonOffer) {
        match self {
            Commands::ListPeers(args) => args.run(daemon).await,
            Commands::ListProcesses(args) => args.run(daemon).await,
            Commands::SpawnWasm(args) => args.run(daemon).await,
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();
    let daemon = hearth_ipc::connect()
        .await
        .expect("Failed to connect to Hearth daemon");
    args.command.run(daemon).await;
}
