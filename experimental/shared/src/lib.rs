//! The shared memory between Delta and its in-game hook (`Local\DeltaOverlayFrame`).
//!
//! Delta renders the overlay into pixels here; the hook, loaded in a game, copies them into the
//! game's frame. The hook also reports which game it's in and its back buffer size, so Delta can lay
//! the overlay out for that resolution.
//!
//! Sprites are written under a sequence lock: Delta makes `seq` odd while writing and even when
//! done; the hook copies only when `seq` is even and the same before and after its copy.

#![no_std]

use core::sync::atomic::{AtomicU32, AtomicU64};

pub const NAME: &str = "Local\\DeltaOverlayFrame";
pub const MAGIC: u32 = 0x41544C44; // "DLTA"
pub const VERSION: u32 = 1;
pub const MAX_SPRITES: usize = 16;
pub const MAX_HOOKS: usize = 16;
pub const PIXEL_BYTES: usize = 24 * 1024 * 1024;
pub const TOTAL: usize = core::mem::size_of::<Header>() + PIXEL_BYTES;

/// One image to draw, premultiplied BGRA rows at `offset` in the pixel area.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Sprite {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub offset: u32,
    /// Changes whenever the pixels change, so the hook only re-uploads then.
    pub version: u32,
}

pub const API_NONE: u32 = 0;
pub const API_D3D11: u32 = 11;
pub const API_D3D12: u32 = 12;

/// Written by a hook in a game. `pid` 0 = free slot.
#[repr(C)]
pub struct HookSlot {
    pub pid: AtomicU32,
    pub api: AtomicU32,
    /// Back buffer size.
    pub width: AtomicU32,
    pub height: AtomicU32,
    /// GetTickCount64 of the last Present the hook saw.
    pub last_present: AtomicU64,
    /// Frames presented (for the FPS of games ETW can't see).
    pub frames: AtomicU64,
}

#[repr(C)]
pub struct Header {
    pub magic: AtomicU32,
    pub version: AtomicU32,
    pub host_pid: AtomicU32,
    /// GetTickCount64 of Delta's last update; hooks stop drawing when it goes stale.
    pub host_heartbeat: AtomicU64,
    /// The process the sprites are meant for (0 = draw nothing).
    pub target_pid: AtomicU32,
    pub seq: AtomicU32,
    pub sprite_count: AtomicU32,
    pub _pad: u32,
    pub sprites: [Sprite; MAX_SPRITES],
    pub hooks: [HookSlot; MAX_HOOKS],
}

/// Pixel area right after the header.
pub const PIXELS_OFFSET: usize = core::mem::size_of::<Header>();

/// Host heartbeat older than this (ms) means Delta is gone or paused: draw nothing.
pub const STALE_MS: u64 = 1500;
