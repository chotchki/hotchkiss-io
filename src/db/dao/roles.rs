use serde::{Deserialize, Serialize};
use strum::{Display, EnumIter, EnumString};

#[derive(
    Clone, Copy, Debug, Display, Deserialize, Eq, EnumIter, EnumString, PartialEq, Serialize,
)]
pub enum Roles {
    Admin,
    Anonymous,
    Registered,
}
