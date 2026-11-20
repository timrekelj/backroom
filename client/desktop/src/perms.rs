//! What the signed-in user may do in a guild, computed the way the server does (`proto/README.md`):
//! the owner has every permission; anyone else has the OR of their roles' permissions, and
//! ADMINISTRATOR implies all. The client only uses this to decide which controls to show; the
//! server still checks every call.

use thehideout_api::{Guild, Permissions, Role};

/// Every bit set, which is what the owner holds.
pub const ALL: Permissions = Permissions(u64::MAX);

/// `me`'s effective permissions in `guild`, given the guild's roles and the role ids they hold.
/// Unknown role ids (a role deleted since) contribute nothing.
pub fn effective(guild: &Guild, me: u64, my_roles: &[u64], roles: &[Role]) -> Permissions {
    if guild.owner_id == me {
        return ALL;
    }
    roles
        .iter()
        .filter(|r| my_roles.contains(&r.id))
        .fold(Permissions::NONE, |p, r| p | r.permissions)
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use thehideout_api::PaletteColor;

    use super::*;

    fn guild(owner: u64) -> Guild {
        Guild {
            id: 1,
            name: "g".into(),
            owner_id: owner,
            invite_code: String::new(),
            created_at: UNIX_EPOCH,
            icon: None,
        }
    }

    fn role(id: u64, permissions: Permissions) -> Role {
        Role {
            id,
            guild_id: 1,
            name: format!("r{id}"),
            color: PaletteColor::None,
            position: 0,
            permissions,
            created_at: UNIX_EPOCH,
        }
    }

    #[test]
    fn the_owner_can_do_everything() {
        assert!(effective(&guild(7), 7, &[], &[]).contains(Permissions::MANAGE_CHANNELS));
    }

    #[test]
    fn roles_add_up_and_strangers_have_nothing() {
        let roles = [
            role(1, Permissions::MANAGE_ROLES),
            role(2, Permissions::MANAGE_CHANNELS),
            role(3, Permissions::MANAGE_SERVER),
        ];
        let p = effective(&guild(9), 7, &[1, 2, 99], &roles);
        assert!(p.contains(Permissions::MANAGE_CHANNELS | Permissions::MANAGE_ROLES));
        assert!(
            !p.contains(Permissions::MANAGE_SERVER),
            "role 3 is not theirs"
        );
        assert!(effective(&guild(9), 7, &[], &roles).is_empty());
    }

    #[test]
    fn administrator_implies_all() {
        let roles = [role(1, Permissions::ADMINISTRATOR)];
        assert!(effective(&guild(9), 7, &[1], &roles).contains(Permissions::MANAGE_CHANNELS));
    }
}
