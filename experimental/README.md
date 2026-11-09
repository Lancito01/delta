# Shelved: Delta's own in-game hook

Delta draws inside exclusive-fullscreen games through RivaTuner Statistics Server instead (see the
main README), so it never loads anything into games itself. This folder keeps the earlier native
approach in case a real gap shows up later. None of it is built with Delta.

- `hook/` - `delta_hook.dll`: detours DXGI Present and draws Delta's overlay in D3D11 games, and in
  D3D12 games through D3D11On12. Tested working in both with `d3dtest`.
- `shared/` - the shared-memory layout between Delta and the hook.
- `d3dtest/` - a tiny D3D11/D3D12 window for testing the hook (`d3dtest d3d12 --load delta_hook.dll`).
- `inject.rs` - loading the hook into a game (LoadLibraryW on a remote thread).
- `anticheat.rs` - spotting anti-cheat before loading anything into a game.

To build them: `cargo build --manifest-path experimental/hook/Cargo.toml` (the hook's crate points
at `../shared`). `host_frame_buffer.rs` is Delta's side: it rendered the overlay offscreen and published it to the
hook through shared memory.
