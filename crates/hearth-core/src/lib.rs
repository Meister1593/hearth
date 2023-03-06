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

use tracing::{debug, error, info, Level};
use tracing_subscriber::prelude::*;

pub use hearth_rpc::remoc::rtc::async_trait;

/// Asset loading and storage.
pub mod asset;

/// Lump loading and storage.
pub mod lump;

/// Process interfaces and message routing.
pub mod process;

/// Peer runtime building and execution.
pub mod runtime;

/// Helper function to set up console logging with reasonable defaults.
pub fn init_logging() {
    let filter = tracing_subscriber::filter::Targets::new()
        .with_target("wgpu", Level::INFO)
        .with_target("wgpu_core", Level::WARN)
        .with_target("wgpu_hal", Level::WARN)
        .with_default(Level::DEBUG);

    let format = tracing_subscriber::fmt::layer().compact();

    tracing_subscriber::registry()
        .with(filter)
        .with(format)
        .init();
}

/// Helper function to wait for Ctrl+C with nice logging.
pub async fn wait_for_interrupt() {
    debug!("Waiting for interrupt signal");
    match tokio::signal::ctrl_c().await {
        Ok(()) => info!("Interrupt signal received"),
        Err(err) => error!("Interrupt await error: {:?}", err),
    }
}
