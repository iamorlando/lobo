/// Native policy combinations, selected once when a book is constructed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "server-api",
    derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)
)]
#[cfg_attr(feature = "server-api", serde(rename_all = "snake_case"))]
pub enum BookPolicy {
    #[default]
    Full,
    NoUserMap,
    NoHiddenQuantity,
    NoUpdates,
}

impl BookPolicy {
    pub const fn from_flags(update_user_map: bool, update_hidden: bool) -> Self {
        match (update_user_map, update_hidden) {
            (true, true) => Self::Full,
            (false, true) => Self::NoUserMap,
            (true, false) => Self::NoHiddenQuantity,
            (false, false) => Self::NoUpdates,
        }
    }
    pub const fn update_user_map(self) -> bool {
        matches!(self, Self::Full | Self::NoHiddenQuantity)
    }
    pub const fn update_hidden(self) -> bool {
        matches!(self, Self::Full | Self::NoUserMap)
    }
}

/// Checksum policy selected when a book is constructed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "lobo", eq, frozen, hash, from_py_object)
)]
#[cfg_attr(
    feature = "server-api",
    derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)
)]
#[cfg_attr(feature = "server-api", serde(rename_all = "snake_case"))]
pub enum CheckSum {
    #[default]
    Null,
    BitFinex,
    Kraken,
}
