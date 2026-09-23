// archive/meta/manifest 跨平台：wind-packer 仅依赖这些，可在 Linux 原生构建。
pub mod archive;
pub mod manifest;
pub mod meta;

// installer/uninstaller/ui/util 依赖 windows/winreg/mslnk，仅 Windows 构建。
#[cfg(windows)]
pub mod installer;
#[cfg(windows)]
pub mod ui;
#[cfg(windows)]
pub mod uninstaller;
#[cfg(windows)]
#[allow(dead_code)]
pub mod util;
