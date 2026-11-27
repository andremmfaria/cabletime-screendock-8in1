mod follow_x11;
#[path = "../support/mod.rs"]
mod support;
#[cfg(feature = "capture-wayland")]
mod virtual_mutter;
mod virtual_sway;
mod virtual_x11;
mod x11;
