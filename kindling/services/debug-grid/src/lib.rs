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

use glam::vec3;
use hearth_guest::{debug_draw::*, *};

#[no_mangle]
pub extern "C" fn run() {
    let dd_factory = REGISTRY.get_service("hearth.DebugDrawFactory").unwrap();

    let size = 15;
    let color = Color::from_rgb(0x6a, 0xf5, 0xfc);
    let grid_to_pos = |x: i32, y: i32| vec3(x as f32 * 5.0, -8.0, y as f32 * 5.0);

    let mut vertices = Vec::new();

    for x in -size..=size {
        vertices.push(DebugDrawVertex {
            position: grid_to_pos(x, -size),
            color,
        });

        vertices.push(DebugDrawVertex {
            position: grid_to_pos(x, size),
            color,
        });
    }

    for y in -size..=size {
        vertices.push(DebugDrawVertex {
            position: grid_to_pos(-size, y),
            color,
        });

        vertices.push(DebugDrawVertex {
            position: grid_to_pos(size, y),
            color,
        });
    }

    let reply = Mailbox::new();
    let reply_cap = reply.make_capability(Permissions::SEND);
    dd_factory.send_json(&(), &[&reply_cap]);
    let ((), caps) = reply.recv_json();
    let dd = caps.get(0).unwrap().clone();

    dd.send_json(
        &DebugDrawUpdate::Contents(DebugDrawMesh {
            indices: (0..(vertices.len() as u32)).collect(),
            vertices,
        }),
        &[],
    );
}
