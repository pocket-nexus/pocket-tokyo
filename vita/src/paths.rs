//! Where files live on the console and on the development computer.

/// The USB share's folder for this app, when the wired debug host runs.
pub const HOST: &str = "host0:tokyo";
pub const GXP_HOST: &str = "host0:tokyo/gxp";
/// This app's folder on the memory card.
pub const DATA: &str = "ux0:data/pocket-tokyo";
/// Programs compiled on this console.
pub const GXP_CACHE: &str = "ux0:data/pocket-tokyo/gxp";
/// Programs shipped in the package.
pub const GXP_PACKAGED: &str = "app0:gxp";
/// The city pack on the computer, and what says which one it is.
pub const PACK_HOST: &str = "host0:tokyo/city.pack";
pub const PACK_HOST_ID: &str = "host0:tokyo/city.json";
/// The copy of it the console reads from, and which one that is.
pub const PACK_CARD: &str = "ux0:data/pocket-tokyo/city.pack";
pub const PACK_CARD_ID: &str = "ux0:data/pocket-tokyo/city.json";
/// The pack of a packaged build.
pub const PACK_APP: &str = "app0:city.pack";
