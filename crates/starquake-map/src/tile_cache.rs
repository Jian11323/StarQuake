//! 栅格瓦片异步拉取与 LRU 纹理缓存。

use crate::projection::{self, TILE_SIZE};
use egui::{Color32, ColorImage, Context, Rect, TextureHandle, TextureOptions, Ui};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

const DEFAULT_CACHED: usize = 256;
const MAX_CACHED_CAP: usize = 800;
const MAX_IN_FLIGHT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

enum TileState {
    Pending,
    Ready(TextureHandle),
    Failed,
}

struct DecodedTile {
    id: TileId,
    image: Option<ColorImage>,
}

pub struct TileCache {
    tile_base: String,
    tile_source: String,
    entries: HashMap<TileId, TileState>,
    lru: VecDeque<TileId>,
    max_cached: usize,
    tx: Sender<DecodedTile>,
    rx: Receiver<DecodedTile>,
    in_flight: usize,
    want: VecDeque<TileId>,
}

impl TileCache {
    pub fn new(tile_base: String, tile_source: String) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tile_base: tile_base.trim_end_matches('/').to_string(),
            tile_source,
            entries: HashMap::new(),
            lru: VecDeque::new(),
            max_cached: DEFAULT_CACHED,
            tx,
            rx,
            in_flight: 0,
            want: VecDeque::new(),
        }
    }

    pub fn set_source(&mut self, tile_base: String, tile_source: String) {
        let base = tile_base.trim_end_matches('/').to_string();
        if base == self.tile_base && tile_source == self.tile_source {
            return;
        }
        self.tile_base = base;
        self.tile_source = tile_source;
        self.clear();
    }

    /// 瓦片张数上限（内存五档：64…800）。
    pub fn set_capacity(&mut self, slots: usize) {
        self.max_cached = slots.clamp(64, MAX_CACHED_CAP);
        self.evict_overflow();
    }

    /// 清空内存中的瓦片纹理与排队请求（不改 URL）。
    pub fn clear(&mut self) {
        self.entries.clear();
        self.lru.clear();
        self.want.clear();
        self.in_flight = 0;
    }

    fn url(&self, id: TileId) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            self.tile_base, self.tile_source, id.z, id.x, id.y
        )
    }

    fn touch_lru(&mut self, id: TileId) {
        if let Some(pos) = self.lru.iter().position(|k| *k == id) {
            self.lru.remove(pos);
        }
        self.lru.push_back(id);
        self.evict_overflow();
    }

    fn evict_overflow(&mut self) {
        while self.lru.len() > self.max_cached {
            if let Some(old) = self.lru.pop_front() {
                self.entries.remove(&old);
            }
        }
    }

    fn enqueue(&mut self, id: TileId) {
        if self.entries.contains_key(&id) {
            return;
        }
        if self.want.iter().any(|k| *k == id) {
            return;
        }
        self.want.push_back(id);
        self.entries.insert(id, TileState::Pending);
    }

    fn spawn_jobs(&mut self) {
        while self.in_flight < MAX_IN_FLIGHT {
            let Some(id) = self.want.pop_front() else {
                break;
            };
            if !matches!(self.entries.get(&id), Some(TileState::Pending)) {
                continue;
            }
            let url = self.url(id);
            let tx = self.tx.clone();
            self.in_flight += 1;
            let _ = thread::Builder::new()
                .name("tile-fetch".into())
                .spawn(move || {
                    let image = fetch_tile(&url);
                    let _ = tx.send(DecodedTile { id, image });
                });
        }
    }

    pub fn poll(&mut self, ctx: &Context) {
        while let Ok(decoded) = self.rx.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            match decoded.image {
                Some(img) => {
                    let tex = ctx.load_texture(
                        format!("tile-{}-{}-{}", decoded.id.z, decoded.id.x, decoded.id.y),
                        img,
                        TextureOptions::LINEAR,
                    );
                    self.entries.insert(decoded.id, TileState::Ready(tex));
                    self.touch_lru(decoded.id);
                }
                None => {
                    self.entries.insert(decoded.id, TileState::Failed);
                }
            }
            crate::request_repaint_after_capped(ctx, 50);
        }
        self.spawn_jobs();
    }

    /// 请求可见瓦片并绘制（支持非整数 zoom：取 floor(z) 瓦片再按比例缩放）
    pub fn paint(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        center_lon: f64,
        center_lat: f64,
        zoom: f64,
    ) {
        self.poll(ui.ctx());

        let z = zoom.floor().clamp(0.0, 18.0) as u8;
        let zf = z as f64;
        let scale = (2f64.powf(zoom - zf)) as f32;
        let tile_px = (TILE_SIZE as f32) * scale;

        let (cz, x0, y0, x1, y1) =
            projection::visible_tiles(center_lon, center_lat, zoom, rect, 1);
        let _ = cz;
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.enqueue(TileId {
                    z,
                    x: projection::wrap_tile_x(x, z),
                    y: y as u32,
                });
            }
        }
        self.spawn_jobs();

        let painter = ui.painter_at(rect);
        let (cx, cy) = projection::lonlat_to_world(center_lon, center_lat, zf);
        let origin = rect.center();

        for y in y0..=y1 {
            for x in x0..=x1 {
                let id = TileId {
                    z,
                    x: projection::wrap_tile_x(x, z),
                    y: y as u32,
                };
                // 屏幕位置用未折绕的列号，实现横向循环铺贴
                let (twx, twy) = projection::tile_world_origin(x, y, z);
                let screen_min = egui::pos2(
                    origin.x + ((twx - cx) as f32) * scale,
                    origin.y + ((twy - cy) as f32) * scale,
                );
                let tile_rect = Rect::from_min_size(screen_min, egui::vec2(tile_px, tile_px));

                let tex_id = match self.entries.get(&id) {
                    Some(TileState::Ready(tex)) => Some(tex.id()),
                    Some(TileState::Failed) => {
                        painter.rect_filled(tile_rect, 0.0, Color32::from_rgb(24, 28, 34));
                        None
                    }
                    _ => {
                        painter.rect_filled(tile_rect, 0.0, Color32::from_rgb(16, 18, 22));
                        None
                    }
                };
                if let Some(tid) = tex_id {
                    self.touch_lru(id);
                    painter.image(
                        tid,
                        tile_rect,
                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }
        }
    }
}

fn fetch_tile(url: &str) -> Option<ColorImage> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .user_agent("StarQuake/0.1")
        .build()
        .ok()?;
    let resp = match client.get(url).send() {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            tracing::debug!(%url, status = %r.status(), "tile http fail");
            return None;
        }
        Err(e) => {
            tracing::debug!(%url, error = %e, "tile fetch err");
            return None;
        }
    };
    let bytes = resp.bytes().ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}
