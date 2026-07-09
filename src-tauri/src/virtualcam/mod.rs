//! Abstração de webcam virtual: trait `VirtualCameraBackend`
//! (create/destroy/feed_frame/set_standby). Único módulo com código
//! platform-specific (Princípio IV); invariante: 1 fonte ↔ 1 device.

#[cfg(target_os = "windows")]
pub mod akvcam;
#[cfg(target_os = "linux")]
pub mod v4l2;
