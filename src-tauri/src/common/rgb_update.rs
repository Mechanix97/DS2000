use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RGBConfig {
    pub brightness: u8,
    /// How fast the animated modes run, higher being faster. Ignored by `Fixed`, but carried on
    /// every frame regardless so the byte always sits at the same offset.
    pub speed: u8,
    pub rgb_mode: RGBMode,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum RGBMode {
    Rainbow,
    Fixed { led1: LedRgb, led2: LedRgb },
    Breathing { led1: LedRgb, led2: LedRgb },
}

/// Three bytes, so it is `Copy`: passing it by value is cheaper than a reference.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedRgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Default for RGBConfig {
    fn default() -> Self {
        Self {
            brightness: 255,
            // The midpoint, which is the speed the firmware ran at before the control existed.
            speed: 128,
            rgb_mode: RGBMode::Rainbow,
        }
    }
}
