pub mod roots;
pub mod verify;

use serde::{Deserialize, Serialize};

/// TEE platform identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Platform {
    Nitro = 1,
    SevSnp = 2,
    Tdx = 3,
}
