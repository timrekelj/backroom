# TheHideout protocol

Connect/protobuf contract in `proto/thehideout/v1`. Lint: `buf lint`; compatibility: `buf breaking --against '.git#branch=main'`. Changes are additive only.

## Permissions and roles (`RoleService`, `types.proto`)
- `uint64` bit flags (`Permission`): ADMINISTRATOR=1, MANAGE_SERVER=2, MANAGE_CHANNELS=4, MANAGE_ROLES=8.
- The guild owner has every permission; a member's permissions are the OR of their roles; ADMINISTRATOR implies all.
- Hierarchy: non-owners may only manage roles strictly below their highest role (and only grant permissions they hold).
- `RoleService` is separate from `GuildService` because roles have their own version counter and rules. `ListGuildMembersResponse.members` carries role IDs (`users` is kept for compatibility).
- Colours (roles, channels) are one of 12 palette keys: slate red orange amber yellow lime green teal cyan blue indigo pink; `""` = none.

## Channel layout (`GuildService`)
- `ListChannelsResponse.layout` holds the sections and per-section channel order (section 0 = top level); there is no separate GetLayout.
- Structure changes (create/rename/delete section, create channel) bump `GuildLayout.version`. `UpdateLayout` sends the desired order with `expected_version`; a mismatch fails with `ABORTED` (refetch and retry), a non-permutation with `INVALID_ARGUMENT`. `ReorderRoles` works the same with `ListRolesResponse.version`.
- Channel icon: Phosphor name matching `^[a-z0-9]+(-[a-z0-9]+)*$`, ≤48 chars; topic ≤256 chars. All layout edits need MANAGE_CHANNELS.

## Media (`MediaService`)
- `User.avatar` / `Guild.icon` are opaque keys; fetch `GET {base}/media/{key}` (plain HTTP, immutable cache, 404 if unknown).
- Upload ≤ 8 MiB PNG/JPEG/WebP/GIF (first frame); the server centre-crops, resizes to 256×256 and re-encodes (WebP). Guild icons need MANAGE_SERVER.

## Site admin (`AdminService`)
- Every call requires `User.is_admin`. Bootstrap: `THEHIDEOUT_ADMINS` (comma-separated usernames) are promoted at startup and at sign-up.
- `SetAdmin` cannot demote yourself or the last admin (`FAILED_PRECONDITION`).

## Gateway events
New `Event` payloads (fields 7–12): guild_updated, channel_updated, layout_updated, roles_updated (full list), member_updated, user_updated (audience: users sharing a guild or DM with the user).
