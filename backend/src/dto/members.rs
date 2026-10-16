use sea_orm::prelude::DateTimeWithTimeZone;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use lodestone_core::role::Role;

#[derive(Debug, Serialize)]
pub struct MemberResponse {
    pub user_id: Uuid,
    pub email: String,
    pub role: Role,
    pub joined_at: DateTimeWithTimeZone,
}

/// `{"role": "admin"}` — serde refuse toute valeur hors owner/admin/member.
#[derive(Debug, Deserialize)]
pub struct ChangeRoleRequest {
    pub role: Role,
}
