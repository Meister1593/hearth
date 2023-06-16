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

use std::fmt::{Display, Formatter, Result as FmtResult};

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// Paneling-related protocols and utilities.
pub mod panels;

/// WebAssembly process protocols and utilities.
pub mod wasm;

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct ProcessId(pub u64);

impl Display for ProcessId {
    fn fmt(&self, fmt: &mut Formatter) -> FmtResult {
        let (peer_id, local_pid) = self.split();
        write!(fmt, "{}.{}", peer_id.0, local_pid.0)
    }
}

impl std::str::FromStr for ProcessId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (peer_id, local_pid) = s
            .split_once('.')
            .ok_or("Input string does not contain a period")?;

        let peer_id = PeerId(peer_id.parse::<u32>().map_err(|x| x.to_string())?);
        let local_pid = LocalProcessId(local_pid.parse::<u32>().map_err(|x| x.to_string())?);

        Ok(ProcessId::from_peer_process(peer_id, local_pid))
    }
}

impl ProcessId {
    pub fn split(self) -> (PeerId, LocalProcessId) {
        let peer = (self.0 >> 32) as u32;
        let pid = self.0 as u32;
        (PeerId(peer), LocalProcessId(pid))
    }

    pub fn from_peer_process(peer: PeerId, pid: LocalProcessId) -> Self {
        Self(((peer.0 as u64) << 32) | (pid.0 as u64))
    }
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct PeerId(pub u32);

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct LocalProcessId(pub u32);

/// Identifier for a lump (digest of BLAKE3 cryptographic hash).
#[repr(C)]
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, Deserialize, Serialize, Pod, Zeroable)]
pub struct LumpId(pub [u8; 32]);

impl Display for LumpId {
    fn fmt(&self, fmt: &mut Formatter) -> FmtResult {
        for byte in self.0.iter() {
            write!(fmt, "{:02x}", byte)?;
        }

        Ok(())
    }
}

bitflags::bitflags! {
    /// The permission flags of a capability.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Flags: u32 {
        const SEND = 1 << 0;
        const KILL = 1 << 1;
        const REGISTER = 1 << 2;
        const TRUSTED = 1 << 3;
    }
}

/// The severity level for a log message emitted by a process.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, Deserialize, Serialize)]
pub enum ProcessLogLevel {
    Trace,
    Debug,
    Info,
    Warning,
    Error,
}

impl TryFrom<u32> for ProcessLogLevel {
    type Error = ();

    fn try_from(other: u32) -> Result<Self, ()> {
        use ProcessLogLevel::*;
        match other {
            0 => Ok(Trace),
            1 => Ok(Debug),
            2 => Ok(Info),
            3 => Ok(Warning),
            4 => Ok(Error),
            _ => Err(()),
        }
    }
}

impl Into<u32> for ProcessLogLevel {
    fn into(self) -> u32 {
        use ProcessLogLevel::*;
        match self {
            Trace => 0,
            Debug => 1,
            Info => 2,
            Warning => 3,
            Error => 4,
        }
    }
}

#[macro_export]
macro_rules! impl_serialize_json_display {
    ($ty: ident) => {
        impl ::std::fmt::Display for $ty {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                let string = ::serde_json::to_string(self).map_err(|_| ::std::fmt::Error)?;
                f.write_str(&string)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_conversion() {
        let tests = &[(0, 0), (420, 69), (100000, 100000)];
        for (peer, pid) in tests.iter() {
            let peer = PeerId(*peer);
            let pid = LocalProcessId(*pid);
            assert_eq!((peer, pid), ProcessId::from_peer_process(peer, pid).split());
        }
    }
}
