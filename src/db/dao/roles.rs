use serde::{Deserialize, Serialize};
use strum::{Display, EnumIter, EnumString};

#[derive(
    Clone,
    Copy,
    Debug,
    Display,
    Deserialize,
    Eq,
    EnumIter,
    EnumString,
    PartialEq,
    Serialize,
    sqlx::Type,
)]
pub enum Role {
    Admin,
    Anonymous,
    Registered,
}
