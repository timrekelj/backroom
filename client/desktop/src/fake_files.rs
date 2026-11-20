//! The fake world's files: uploads that report progress (and can stall, fail or be cancelled),
//! downloads, and the attachments and embeds the scenes show. It is a child module of `fake`
//! (see the `#[path]` there) so it can reach the world's private fields.
//!
//! Media keys: `fx-img-<seed>-<w>x<h>` is a generated picture of that size, `fx-clip` is a three
//! second 320x180 clip (`assets/clip.mp4`, which `media_path` puts on disk so a player can open it),
//! `fx-file-<n>` is a small text file, and `up-<n>` is what was uploaded.

#![cfg_attr(not(test), allow(dead_code))]

use std::path::PathBuf;

use super::*;
use thehideout_api::{Attachment, Embed, EmbedKind, EmbedMedia, EmbedMediaKind, PendingAttachment};

/// The clip every fake video plays.
pub(super) const CLIP: &[u8] = include_bytes!("../assets/clip.mp4");

#[derive(Default)]
pub(super) struct Files {
    uploads: HashMap<u64, Pending>,
    next_pending: u64,
    /// Uploads stop at this percent and never finish (a progress bar to look at).
    stall: Option<u64>,
    /// Stalled uploads, by client id.
    stalled: Vec<u64>,
    fail_uploads: u32,
    fail_downloads: u32,
    clip_path: Option<PathBuf>,
}

struct Pending {
    filename: String,
    content_type: String,
    bytes: Vec<u8>,
}

/// Whether `key` is one of the generated files.
pub(super) fn is_fixture(key: &str) -> bool {
    key.starts_with("fx-")
}

/// The bytes of a generated file.
pub(super) fn fixture_bytes(key: &str) -> Result<Vec<u8>, ApiError> {
    if let Some(rest) = key.strip_prefix("fx-img-") {
        let (seed, dims) = rest.split_once('-').ok_or(ApiError::NotFound)?;
        let (w, h) = dims.split_once('x').ok_or(ApiError::NotFound)?;
        let (seed, w, h) = (
            seed.parse().map_err(|_| ApiError::NotFound)?,
            w.parse().map_err(|_| ApiError::NotFound)?,
            h.parse().map_err(|_| ApiError::NotFound)?,
        );
        return Ok(crate::media::generated_picture(seed, w, h));
    }
    if key.starts_with("fx-clip") {
        return Ok(CLIP.to_vec());
    }
    if let Some(n) = key.strip_prefix("fx-file-") {
        return Ok(format!("fake file {n}\n").repeat(64).into_bytes());
    }
    Err(ApiError::NotFound)
}

fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

impl World {
    /// Answers the file commands; any other command comes back for `handle`.
    pub(super) fn files_command(&mut self, cmd: Command) -> Result<Vec<Reply>, Command> {
        match cmd {
            Command::UploadFile {
                channel,
                id,
                path,
                filename,
                content_type,
                size: _,
            } => Ok(match std::fs::read(&path) {
                Ok(bytes) => self.upload(channel, id, filename, content_type, bytes),
                Err(e) => vec![Reply::Uploaded {
                    id,
                    result: Err(ApiError::InvalidArgument(format!(
                        "can't read that file: {e}"
                    ))),
                }],
            }),
            Command::UploadBytes {
                channel,
                id,
                filename,
                content_type,
                bytes,
            } => Ok(self.upload(channel, id, filename, content_type, bytes)),
            Command::CancelUpload { id } => {
                self.files.stalled.retain(|s| *s != id);
                self.queue.retain(|(_, r)| !matches!(r, Reply::UploadProgress { id: i, .. } | Reply::UploadPreview { id: i, .. } | Reply::Uploaded { id: i, .. } if *i == id));
                Ok(Vec::new())
            }
            Command::SendWith {
                channel,
                content,
                nonce,
                attachments,
            } => Ok(self.send_with(channel, content, nonce, attachments)),
            Command::Download { key, path } => Ok(self.download(key, path)),
            other => Err(other),
        }
    }

    fn upload(
        &mut self,
        _channel: u64,
        id: u64,
        filename: String,
        content_type: String,
        bytes: Vec<u8>,
    ) -> Vec<Reply> {
        let total = bytes.len() as u64;
        let mut out = Vec::new();
        if content_type.starts_with("image/") {
            out.push(Reply::UploadPreview {
                id,
                bytes: bytes.clone(),
            });
        }
        if self.files.fail_uploads > 0 {
            self.files.fail_uploads -= 1;
            out.push(Reply::UploadProgress {
                id,
                sent: total / 4,
                total,
            });
            out.push(Reply::Uploaded {
                id,
                result: Err(ApiError::Network("simulated outage".into())),
            });
            return out;
        }
        if total > thehideout_api::DEFAULT_MAX_UPLOAD_BYTES {
            out.push(Reply::Uploaded {
                id,
                result: Err(ApiError::TooLarge),
            });
            return out;
        }
        let steps: &[u64] = match self.files.stall {
            Some(p) => {
                self.files.stalled.push(id);
                &[p]
            }
            None => &[25, 50, 75, 100],
        };
        out.extend(steps.iter().map(|p| Reply::UploadProgress {
            id,
            sent: total * p / 100,
            total,
        }));
        if self.files.stall.is_some() {
            return out;
        }
        self.files.next_pending += 1;
        let pending = 9000 + self.files.next_pending;
        let size = total;
        self.files.uploads.insert(
            pending,
            Pending {
                filename: filename.clone(),
                content_type: content_type.clone(),
                bytes,
            },
        );
        out.push(Reply::Uploaded {
            id,
            result: Ok(PendingAttachment {
                id: pending,
                filename,
                content_type,
                size,
                expires_at: time(now_unix() + 3600),
            }),
        });
        out
    }

    fn send_with(
        &mut self,
        channel: u64,
        content: String,
        nonce: String,
        ids: Vec<u64>,
    ) -> Vec<Reply> {
        let me = self.me.id;
        let result = if self.fail_sends > 0 {
            self.fail_sends -= 1;
            Err(ApiError::Network("simulated outage".into()))
        } else if ids.is_empty() && content.trim().is_empty() {
            Err(ApiError::InvalidArgument(
                "message must be 1 to 4000 characters".into(),
            ))
        } else if ids.len() > crate::attach::MAX_FILES {
            Err(ApiError::InvalidArgument(
                "a message carries at most 10 files".into(),
            ))
        } else if let Some(missing) = ids.iter().find(|i| !self.files.uploads.contains_key(i)) {
            let _ = missing;
            Err(ApiError::NotFound)
        } else {
            let mut atts = Vec::new();
            for pid in &ids {
                let p = self.files.uploads.remove(pid).expect("checked above");
                self.next_id += 1;
                let key = format!("up-{pid}");
                let dims = dimensions(&p.bytes).filter(|_| p.content_type.starts_with("image/"));
                atts.push(Attachment {
                    id: self.next_id,
                    filename: p.filename,
                    content_type: p.content_type,
                    size: p.bytes.len() as u64,
                    file: Some(key.clone()),
                    width: dims.map(|d| d.0),
                    height: dims.map(|d| d.1),
                    thumbnail: None,
                    removed: false,
                });
                self.media.insert(key, p.bytes);
            }
            self.sent_at += 15;
            self.push(channel, me, self.sent_at, content.trim());
            let m = self
                .messages
                .get_mut(&channel)
                .and_then(|v| v.last_mut())
                .unwrap();
            m.nonce = nonce.clone();
            m.attachments = atts;
            Ok(m.clone())
        };
        let echo = result
            .clone()
            .ok()
            .map(|m| Reply::Gateway(GatewayUpdate::Event(Event::MessageCreated(m))));
        std::iter::once(Reply::Sent {
            channel,
            nonce,
            result,
        })
        .chain(echo)
        .collect()
    }

    fn download(&mut self, key: String, path: PathBuf) -> Vec<Reply> {
        if self.files.fail_downloads > 0 {
            self.files.fail_downloads -= 1;
            return vec![Reply::Downloaded {
                key,
                path,
                result: Err(ApiError::Network("simulated outage".into())),
            }];
        }
        let bytes = match self.media.get(&key) {
            Some(b) => Ok(b.clone()),
            None => fixture_bytes(&key),
        };
        match bytes {
            Ok(bytes) => match std::fs::write(&path, &bytes) {
                Ok(()) => {
                    let n = bytes.len() as u64;
                    vec![
                        Reply::DownloadProgress {
                            key: key.clone(),
                            received: n / 2,
                            total: Some(n),
                        },
                        Reply::Downloaded {
                            key,
                            path,
                            result: Ok(n),
                        },
                    ]
                }
                Err(e) => vec![Reply::Downloaded {
                    key,
                    path,
                    result: Err(ApiError::Other(format!("write failed: {e}"))),
                }],
            },
            Err(e) => vec![Reply::Downloaded {
                key,
                path,
                result: Err(e),
            }],
        }
    }

    /// Where `key`'s file is on this machine: the clip is written to the temp directory once.
    pub(super) fn local_path(&mut self, key: &str) -> Option<PathBuf> {
        if !key.starts_with("fx-clip") {
            return None;
        }
        if self.files.clip_path.is_none() {
            let path = std::env::temp_dir()
                .join(format!("thehideout-fake-clip-{}.mp4", std::process::id()));
            std::fs::write(&path, CLIP).ok()?;
            self.files.clip_path = Some(path);
        }
        self.files.clip_path.clone()
    }

    fn file(
        &mut self,
        name: &str,
        ct: &str,
        size: u64,
        key: &str,
        dims: Option<(u32, u32)>,
    ) -> Attachment {
        self.next_id += 1;
        Attachment {
            id: self.next_id,
            filename: name.into(),
            content_type: ct.into(),
            size,
            file: Some(key.into()),
            width: dims.map(|d| d.0),
            height: dims.map(|d| d.1),
            thumbnail: None,
            removed: false,
        }
    }

    fn post(
        &mut self,
        channel: u64,
        author: u64,
        content: &str,
        attachments: Vec<Attachment>,
        embeds: Vec<Embed>,
    ) {
        self.sent_at += 20;
        self.push(channel, author, self.sent_at, content);
        let m = self
            .messages
            .get_mut(&channel)
            .and_then(|v| v.last_mut())
            .unwrap();
        m.attachments = attachments;
        m.embeds = embeds;
    }

    /// Pictures, a clip, a PDF, an archive, an audio note and a file an admin removed.
    pub(super) fn seed_attachments(&mut self, channel: u64, only: &[&str]) {
        let want = |n: &str| only.is_empty() || only.contains(&n);
        let img = |seed: u64, w: u32, h: u32| format!("fx-img-{seed}-{w}x{h}");
        if want("three") {
        let three = vec![
            self.file(
                "saturday-1.jpg",
                "image/jpeg",
                840_000,
                &img(11, 800, 600),
                Some((800, 600)),
            ),
            self.file(
                "saturday-2.jpg",
                "image/jpeg",
                910_000,
                &img(12, 600, 800),
                Some((600, 800)),
            ),
            self.file(
                "saturday-3.jpg",
                "image/jpeg",
                1_200_000,
                &img(13, 1200, 700),
                Some((1200, 700)),
            ),
        ];
        self.post(channel, ANA, "Photos from Saturday", three, vec![]);
        }
        if want("one") {
        let one = vec![self.file(
            "dashboard.png",
            "image/png",
            420_000,
            &img(21, 1600, 900),
            Some((1600, 900)),
        )];
        self.post(
            channel,
            BRUNO,
            "the new dashboard, before the polish pass",
            one,
            vec![],
        );
        }
        if want("four") {
        let four: Vec<Attachment> = (0..4)
            .map(|i| {
                self.file(
                    &format!("mood-{}.png", i + 1),
                    "image/png",
                    300_000,
                    &img(30 + i, 640, 480),
                    Some((640, 480)),
                )
            })
            .collect();
        self.post(channel, CVETKA, "moodboard", four, vec![]);
        }
        if want("video") {
        let clip = vec![self.file("demo.mp4", "video/mp4", 34_640, "fx-clip", Some((320, 180)))];
        self.post(channel, ANA, "screen recording of the bug", clip, vec![]);
        }
        if want("docs") {
        let docs = vec![
            self.file(
                "Q4-roadmap.pdf",
                "application/pdf",
                2_516_582,
                "fx-file-1",
                None,
            ),
            self.file(
                "assets.zip",
                "application/zip",
                18_874_368,
                "fx-file-2",
                None,
            ),
        ];
        self.post(channel, BRUNO, "roadmap and the asset bundle", docs, vec![]);
        }
        if want("audio") {
        let audio = vec![self.file("voice-note.m4a", "audio/mp4", 34_640, "fx-clip-audio", None)];
        self.post(channel, ANA, "", audio, vec![]);
        }
        if want("removed") {
        let mut gone = self.file(
            "invoice.xlsx",
            "application/vnd.ms-excel",
            88_000,
            "fx-file-3",
            None,
        );
        gone.file = None;
        gone.removed = true;
        self.post(
            channel,
            CVETKA,
            "this was the wrong sheet",
            vec![gone],
            vec![],
        );
        }
    }

    /// A link, and a post from each provider: X (video, then a looping GIF), TikTok, Instagram, a
    /// Reddit gallery, and a video the server did not keep (poster only).
    pub(super) fn seed_embeds(&mut self, channel: u64, only: &[&str]) {
        let want = |n: &str| only.is_empty() || only.contains(&n);
        let img = |seed: u64, w: u32, h: u32| format!("fx-img-{seed}-{w}x{h}");
        let base = |url: &str| Embed {
            url: url.into(),
            title: String::new(),
            description: String::new(),
            site_name: String::new(),
            thumbnail: None,
            thumbnail_width: None,
            thumbnail_height: None,
            kind: EmbedKind::Link,
            provider: String::new(),
            author_name: String::new(),
            author_url: None,
            author_icon: None,
            media: vec![],
            published_at: None,
        };
        let media =
            |kind: EmbedMediaKind, file: &str, poster: Option<String>, w: u32, h: u32| EmbedMedia {
                kind,
                file: file.into(),
                poster,
                width: w,
                height: h,
                duration_ms: Some(3000),
                size: 34_640,
            };
        let at = |d: u32, h: i64, m: i64| Some(time(utc(2026, 11, d, h, m)));

        if want("link") {
        let mut link = base("https://github.com/anomalyco/rune");
        link.title = "anomalyco/rune: a retained-mode UI toolkit".into();
        link.description = "Rune draws the window, handles input and plays the video. Springs for every motion, a headless runtime for tests, and a design system that stays out of the way of the content. The README walks through a first app in about fifty lines.".into();
        link.site_name = "GitHub".into();
        link.thumbnail = Some(img(41, 1200, 630));
        link.thumbnail_width = Some(1200);
        link.thumbnail_height = Some(630);
        self.post(
            channel,
            BRUNO,
            "have a look at this https://github.com/anomalyco/rune",
            vec![],
            vec![link],
        );
        }

        if want("x") {
        let mut x = base("https://x.com/rune_ui/status/1");
        x.kind = EmbedKind::Video;
        x.provider = "x".into();
        x.author_name = "Rune".into();
        x.author_icon = Some(img(51, 64, 64));
        x.description = "Video playback lands in Rune: AVFoundation on macOS, Media Foundation on Windows, zero copy.\n\nNo web view, no codec bundle.".into();
        x.media = vec![media(
            EmbedMediaKind::Video,
            "fx-clip-x",
            Some(img(52, 640, 360)),
            640,
            360,
        )];
        x.published_at = at(19, 14, 3);
        self.post(
            channel,
            ANA,
            "https://x.com/rune_ui/status/1",
            vec![],
            vec![x],
        );
        }

        if want("gif") {
        let mut gif = base("https://x.com/rune_ui/status/2");
        gif.kind = EmbedKind::Video;
        gif.provider = "x".into();
        gif.author_name = "Rune".into();
        gif.author_icon = Some(img(51, 64, 64));
        gif.description = "the spring, slowed down".into();
        gif.media = vec![media(
            EmbedMediaKind::Gifv,
            "fx-clip-gif",
            Some(img(53, 480, 270)),
            480,
            270,
        )];
        gif.published_at = at(19, 15, 20);
        self.post(
            channel,
            BRUNO,
            "https://x.com/rune_ui/status/2",
            vec![],
            vec![gif],
        );
        }

        if want("tiktok") {
        let mut tt = base("https://www.tiktok.com/@maker/video/1");
        tt.kind = EmbedKind::Video;
        tt.provider = "tiktok".into();
        tt.author_name = "maker".into();
        tt.author_icon = Some(img(61, 64, 64));
        tt.description = "building a desk in one take #woodworking".into();
        tt.media = vec![media(
            EmbedMediaKind::Video,
            "fx-clip-tt",
            Some(img(62, 360, 640)),
            360,
            640,
        )];
        tt.published_at = at(18, 9, 41);
        self.post(
            channel,
            CVETKA,
            "https://www.tiktok.com/@maker/video/1",
            vec![],
            vec![tt],
        );
        }

        if want("instagram") {
        let mut ig = base("https://www.instagram.com/p/abc/");
        ig.kind = EmbedKind::Image;
        ig.provider = "instagram".into();
        ig.author_name = "studio.nook".into();
        ig.author_icon = Some(img(71, 64, 64));
        ig.description = "Morning light in the new studio.".into();
        ig.media = vec![media(
            EmbedMediaKind::Image,
            &img(72, 1080, 1080),
            None,
            1080,
            1080,
        )];
        ig.published_at = at(17, 8, 12);
        self.post(
            channel,
            ANA,
            "https://www.instagram.com/p/abc/",
            vec![],
            vec![ig],
        );
        }

        if want("reddit") {
        let mut rd = base("https://www.reddit.com/r/battlestations/comments/xyz/");
        rd.kind = EmbedKind::Rich;
        rd.provider = "reddit".into();
        rd.title = "Finally finished the desk setup".into();
        rd.author_name = "u/deskgoblin".into();
        rd.description =
            "Three years in the making. Walnut top, **cable tray**, and a shelf for the plants."
                .into();
        rd.media = (0..4)
            .map(|i| {
                media(
                    EmbedMediaKind::Image,
                    &img(80 + i, 960, 720),
                    None,
                    960,
                    720,
                )
            })
            .collect();
        rd.published_at = at(16, 20, 5);
        self.post(
            channel,
            BRUNO,
            "https://www.reddit.com/r/battlestations/comments/xyz/",
            vec![],
            vec![rd],
        );
        }

        if want("big") {
        let mut big = base("https://www.tiktok.com/@maker/video/2");
        big.kind = EmbedKind::Video;
        big.provider = "tiktok".into();
        big.author_name = "maker".into();
        big.description = "the full 40 minute build".into();
        big.media = vec![media(
            EmbedMediaKind::Video,
            "",
            Some(img(91, 640, 360)),
            640,
            360,
        )];
        big.published_at = at(15, 12, 0);
        self.post(
            channel,
            CVETKA,
            "https://www.tiktok.com/@maker/video/2",
            vec![],
            vec![big],
        );
        }
    }

    /// The server resolved `embeds` for message `id`: the gateway says so.
    pub(super) fn update_embeds(&mut self, channel: u64, id: u64, embeds: Vec<Embed>) {
        if let Some(m) = self
            .messages
            .get_mut(&channel)
            .and_then(|v| v.iter_mut().find(|m| m.id == id))
        {
            m.embeds = embeds;
            let m = m.clone();
            self.enqueue(Reply::Gateway(GatewayUpdate::Event(Event::MessageUpdated(
                m,
            ))));
        }
    }
}

impl FakeBackend {
    /// Uploads stop at `percent` and never finish.
    pub fn stall_uploads(&self, percent: Option<u64>) {
        self.0.borrow_mut().files.stall = percent;
    }

    /// The next `n` uploads fail.
    pub fn fail_uploads(&self, n: u32) {
        self.0.borrow_mut().files.fail_uploads = n;
    }

    pub fn fail_downloads(&self, n: u32) {
        self.0.borrow_mut().files.fail_downloads = n;
    }

    /// Seeds `channel` with attachments: `only` names which (three, one, four, video, docs, audio,
    /// removed); empty means all (for the scenes).
    pub fn seed_attachments(&self, channel: u64, only: &[&str]) {
        self.0.borrow_mut().seed_attachments(channel, only);
    }

    /// Seeds `channel` with embeds: `only` names which (link, x, gif, tiktok, instagram, reddit,
    /// big); empty means all (for the scenes).
    pub fn seed_embeds(&self, channel: u64, only: &[&str]) {
        self.0.borrow_mut().seed_embeds(channel, only);
    }

    /// `embeds` arrive on message `id`, as they do a moment after it was sent.
    pub fn update_embeds(&self, channel: u64, id: u64, embeds: Vec<Embed>) {
        self.0.borrow_mut().update_embeds(channel, id, embeds);
    }

    /// Posts a message that links to a page, with no embed yet, as the gateway delivers it;
    /// returns its id.
    pub fn post_link(&self, channel: u64, author: u64, content: &str) -> u64 {
        let mut w = self.0.borrow_mut();
        w.incoming(channel, author, content);
        w.messages[&channel].last().map(|m| m.id).unwrap_or_default()
    }

    /// An embed made for tests: a plain link or a provider post with `media`.
    pub fn link_embed(url: &str, title: &str) -> Embed {
        Embed {
            url: url.into(),
            title: title.into(),
            description: "A page about something.".into(),
            site_name: "Example".into(),
            thumbnail: None,
            thumbnail_width: None,
            thumbnail_height: None,
            kind: EmbedKind::Link,
            provider: String::new(),
            author_name: String::new(),
            author_url: None,
            author_icon: None,
            media: vec![],
            published_at: None,
        }
    }

    /// A GIFV post (a looping clip) for tests.
    pub fn gifv_embed() -> Embed {
        Embed {
            kind: EmbedKind::Video,
            provider: "x".into(),
            author_name: "Rune".into(),
            media: vec![EmbedMedia {
                kind: EmbedMediaKind::Gifv,
                file: "fx-clip".into(),
                poster: None,
                width: 320,
                height: 180,
                duration_ms: Some(3000),
                size: 34_640,
            }],
            ..FakeBackend::link_embed("https://x.com/a/status/9", "")
        }
    }
}
