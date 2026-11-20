# TheHideout protocol

Connect/protobuf contract in `proto/thehideout/v1`. Lint: `buf lint`; compatibility: `buf breaking --against '.git#branch=main'`. Changes are additive only.

## Permissions and roles (`RoleService`, `types.proto`)
- `uint64` bit flags (`Permission`): ADMINISTRATOR=1, MANAGE_SERVER=2, MANAGE_CHANNELS=4, MANAGE_ROLES=8, MANAGE_EMOJIS=16 (owner / ADMINISTRATOR imply it).
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

## Custom emojis (`EmojiService`, `emoji_service.proto`)
- `Emoji { id, guild_id, name, animated, image, creator_id, created_at }`; `image` is a media key (`GET /media/{key}`). RPCs: ListEmojis (any member), CreateEmoji / RenameEmoji / DeleteEmoji (MANAGE_EMOJIS). Every mutation publishes `emojis_updated { guild_id, emojis }` (the full list).
- Names match `^[a-z0-9_]{2,32}$`, unique per guild (`ALREADY_EXISTS`). Max 200 per guild (`RESOURCE_EXHAUSTED`).
- Input ≤ 2 MiB PNG/JPEG/WebP/GIF; output fits within 128×128 (aspect kept, not cropped, never upscaled). Static images are stored as WebP, animated GIF/WebP keep their animation and are stored as GIF (`animated = true`).
- Message tokens (message `content` stays opaque text; clients parse these): custom emoji `<:name:id>`, animated `<a:name:id>`. In a guild channel only that guild's emojis may be used; in DMs only Unicode emoji. `id` is authoritative, `name` is a fallback label.

## Channel icons (`ChannelIcon`)
- `ChannelIcon { oneof kind { phosphor | unicode | custom_emoji_id } }`. `Channel.icon_ref` (field 10) is authoritative; unset = default icon.
- `Channel.icon` (string) is a deprecated mirror: the Phosphor name when kind = phosphor, `""` otherwise, so old readers keep working.
- `CreateChannelRequest.icon_ref` / `UpdateChannelRequest.icon_ref` are optional; if both `icon` and `icon_ref` are set, `icon_ref` wins. Validation: phosphor as above; unicode = exactly one emoji grapheme cluster, ≤ 32 bytes; custom = an emoji of the same guild. Failures are `INVALID_ARGUMENT`.

## Reactions (`MessageService`)
- `Message.reactions: [Reaction { emoji: ReactionEmoji{unicode|custom_emoji_id}, count, me }]`. `AddReaction` / `RemoveReaction(channel_id, message_id, emoji)` are idempotent.
- Max 20 distinct emojis per message (`RESOURCE_EXHAUSTED`). A custom emoji must belong to the channel's guild; none are valid in DMs (`INVALID_ARGUMENT`).
- Events `reaction_added` / `reaction_removed { channel_id, message_id, emoji, user_id }`. `me` is only meaningful in API responses; clients derive it from events whose `user_id` is their own.

## Attachments (`attachment.proto`, `Message.attachments`)
- Upload is plain HTTP: `POST {base}/attachments?channel_id=<id>&filename=<urlencoded>` with `Authorization: Bearer <token>`, `Content-Type: <mime>` and the raw file as body, ≤ `THEHIDEOUT_MAX_UPLOAD_BYTES` (default 50 MiB). `200` returns JSON `PendingAttachment { id, filename, content_type, size, expires_at }`. Errors: 401 no/invalid token, 403 caller cannot post in the channel, 404 unknown channel, 413 too large, 429 too many pending uploads. A channel you cannot access (not a member of its guild, not a DM participant) is reported as 404 `channel not found` — not 403 — so a channel's existence is not leaked to outsiders.
- A pending upload expires after 1 hour if unused. `SendMessageRequest.attachment_ids` (≤ 10) must be the caller's pending, unexpired uploads for that same channel (`INVALID_ARGUMENT` otherwise). `content` may be empty when attachments are present.
- `Attachment { id, filename, content_type, size, file, width?, height?, thumbnail?, removed }`. Images get width/height and a WebP thumbnail ≤ 400 px (media key). `removed = true` after an admin deletion (`file` / `thumbnail` empty).
- `/media/{key}` serves attachment keys; keys are random and unguessable (≥ 128 bits), which is the access control. `Content-Disposition` is `inline` for images, video and PDF, `attachment; filename=…` for everything else.

## Link embeds (`Message.embeds`)
- `Embed { url, title, description, site_name, thumbnail?, thumbnail_width?, thumbnail_height? }`. The server resolves up to 3 http(s) URLs per message asynchronously, after the message is created, then publishes `message_updated { message }` (full message). `message_updated` is also published when an attachment is removed or embeds change. URLs inside inline code or code fences (unclosed ones included) are not embedded.
- SSRF rules: only http/https; resolved addresses must not be private, loopback, link-local, unspecified, multicast or otherwise non-public (checked on every redirect hop and on the connected IP); short connect/total timeouts; capped redirects and response size; only HTML is parsed. Embed thumbnails are re-encoded to WebP, stored in S3 and counted as the owning guild's storage.

## Rich provider embeds (`Embed.kind` / `provider` / `author_*` / `media` / `published_at`)
- Additive fields 8–14 on `Embed`: `kind` (`EmbedKind`: UNSPECIFIED / LINK / IMAGE / VIDEO / RICH; UNSPECIFIED from servers that predate rich embeds — treat as LINK), `provider` (`"x"`, `"tiktok"`, `"instagram"` or `"reddit"`; `""` = generic link), `author_name`, `author_url?`, `author_icon?` (media key of a small WebP avatar), `media` (repeated `EmbedMedia`) and `published_at?`. `EmbedMedia { kind (IMAGE|VIDEO|GIFV), file, poster?, width, height, duration_ms?, size }`; `file`/`poster` are media keys served from `/media/{key}`. The message text keeps the original link — mirrors are only used for fetching.
- X/Twitter, TikTok, Instagram and Reddit links (incl. `vm.`/`vt.tiktok.com` and `redd.it` short links, resolved first) are described by site-specific providers that fetch public embed-friendly mirrors/APIs, never the site itself, all through the same SSRF-safe client with a browser-style user agent. Mirrors are tried in order per role until one yields something; results are cached per URL (24 h positive, 1 h negative). Generic links keep the OpenGraph/Twitter-card behaviour above.
- Mirror roles and default base URLs (tried in order):
  - `x_api`: `https://api.fxtwitter.com` (`{base}/status/{id}`); `x_vx_api`: `https://api.vxtwitter.com` (`{base}/{user}/status/{id}`); `x_html`: `https://fxtwitter.com`, `https://fixupx.com` (OpenGraph pages)
  - `tiktok`: `https://tnktok.com`, `https://tiktxk.com` (OpenGraph pages)
  - `instagram`: `https://uuinstagram.com`, `https://eeinstagram.com`, `https://vxinstagram.com`, `https://kkinstagram.com`
  - `reddit`: `https://vxreddit.com`, `https://rxddit.com` (OpenGraph; `vxreddit.com` needs a `Mozilla/5.0 (compatible; …bot)` user agent and returns a muxed mp4); `reddit_json`: `https://www.reddit.com`, `https://old.reddit.com` (`/comments/{id}.json` — 403/login for non-Discord user agents, so it rarely answers)
  - Researched and working: api.fxtwitter.com, api.vxtwitter.com, tnktok.com, tiktxk.com, uuinstagram.com, eeinstagram.com, vxinstagram.com, kkinstagram.com, vxreddit.com. Researched and avoid: vxtiktok.com, tfxktok.com, ddinstagram.com, instagramez.com (ad redirect), rxddit.com.
- Override the lists with `THEHIDEOUT_EMBED_MIRRORS`, a JSON object of role → list of base URLs (roles you leave out keep their defaults; unknown roles or non-http(s) URLs are a startup error), e.g. `{"tiktok":["https://tnktok.com"],"instagram":["https://uuinstagram.com","https://vxinstagram.com"]}`.
- Media storage and caps: provider media is downloaded into S3 under `embeds/{channel_id}/{id}.{webp,mp4,webm}` — each embed owns its objects (never shared) with their own accounting rows, counted as the owning guild's embed storage (DMs: global only, author as owner). Images are re-encoded to WebP (≤ 1600 px on the longer side), ≤ 10 MiB each; at most 4 media per embed. Videos are stored as downloaded (MP4, not transcoded) up to `THEHIDEOUT_EMBED_MAX_VIDEO_BYTES` (default 52428800 = 50 MiB) — a bigger one is not stored and the embed keeps its poster and link; each video also gets a WebP `poster`. GIFs and videos without audio are `GIFV`: a looping silent MP4 clients autoplay muted. Timeouts: 15 s per provider run (all mirrors), 6 s per mirror request, 30 s per media download.
- Serving: `/media/{key}` serves embed media like any media key; MP4 comes out as `video/mp4` with `Content-Disposition: inline` and `Accept-Ranges: bytes` (a `Range` request gets `206` with `Content-Range`).
- Fallbacks: a URL whose every mirror fails, times out or has nothing to show simply gets no embed (silent by design — same as an unreachable generic link); a video over the cap degrades to poster + link; if none of a message's URLs yields an embed, nothing is stored or published.

## Admin files and audit (`AdminService`)
- `StorageBreakdown { attachments, emojis, guild_icons, avatars, embeds }` in bytes: `GetOverviewResponse.storage` (global) and `AdminGuildRow.storage` (per guild; `avatars` = 0).
- `ListFiles(guild_id?, kind?, page_token, limit)` returns `AdminFileRow`s sorted by size descending. `DeleteFile(key, reason)` removes the object and its owner reference: attachment → `removed = true` + `message_updated`; emoji → deleted + `emojis_updated`; guild icon / avatar → cleared + `guild_updated` / `user_updated`; embed thumbnail → cleared + `message_updated`. Counters are adjusted and an audit entry (`file.delete`) is written.
- `ListAuditLog(page_token, limit)` returns `AuditEntry { id, actor, action, target, reason, created_at }`, newest first; `SetAdmin` writes `admin.grant` / `admin.revoke`.

## Gateway events
New `Event` payloads (fields 13–16): emojis_updated (full list), reaction_added, reaction_removed, message_updated. Earlier (fields 7–12): guild_updated, channel_updated, layout_updated, roles_updated (full list), member_updated, user_updated (audience: users sharing a guild or DM with the user).
