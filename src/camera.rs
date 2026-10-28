use crate::{
    camera_model::{CameraConfig, CameraPreset, Layer, LayerKind, Scene},
    widgets,
};
use eframe::egui::{self, Color32, RichText, Vec2};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
enum Transport {
    Pipe(ChildStdin),
    Socket(UnixStream),
}
impl Write for Transport {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Pipe(p) => p.write(b),
            Self::Socket(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Pipe(p) => p.flush(),
            Self::Socket(s) => s.flush(),
        }
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        if let Self::Socket(s) = self {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
}
#[derive(Clone, Deserialize)]
struct Device {
    path: String,
    name: String,
}
#[derive(Clone, Deserialize)]
struct Choice {
    id: String,
    name: String,
}
#[derive(Clone, Deserialize)]
struct Control {
    id: String,
    name: String,
    kind: String,
    value: Value,
    default: Value,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
    inactive: bool,
    readonly: bool,
    reopener: bool,
    zeroer: bool,
    unrestorable: bool,
    group: String,
    help: String,
    menu: Vec<Choice>,
}
impl Control {
    fn restorable(&self) -> bool {
        !self.inactive
            && !self.readonly
            && !self.reopener
            && !self.unrestorable
            && !self.zeroer
            && !matches!(self.kind.as_str(), "button" | "info")
    }
}
pub struct Camera {
    pub config: CameraConfig,
    saved_config: CameraConfig,
    modified: bool,
    pub save_pending: bool,
    devices: Vec<Device>,
    controls: Vec<Control>,
    output: String,
    pub running: bool,
    state: String,
    pub error: Option<String>,
    warnings: Vec<String>,
    diagnostics: VecDeque<String>,
    child: Option<Child>,
    stdin: Option<Transport>,
    direct: bool,
    pub virtual_enabled: bool,
    actions: VecDeque<Value>,
    rx: Option<mpsc::Receiver<Value>>,
    initialized: bool,
    visible: bool,
    texture: Option<egui::TextureHandle>,
    preview_size: [usize; 2],
    aspects: BTreeMap<String, f32>,
    frame_pending: bool,
    tab: usize,
    selected_layer: Option<usize>,
    scene_pending: bool,
    last_scene: Instant,
    ptz_active: Option<String>,
}
fn value_text(v: &Value) -> String {
    v.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string())
}
impl Default for Camera {
    fn default() -> Self {
        let mut config: CameraConfig =
            fs::read(crate::preferences::directory().join("camera.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
        let modified = config.scenes.is_empty();
        config.migrate();
        Self {
            saved_config: config.clone(),
            modified,
            save_pending: false,
            config,
            devices: Vec::new(),
            controls: Vec::new(),
            output: String::new(),
            running: false,
            state: "Stopped".into(),
            error: None,
            warnings: Vec::new(),
            diagnostics: VecDeque::new(),
            child: None,
            stdin: None,
            direct: false,
            virtual_enabled: false,
            actions: VecDeque::new(),
            rx: None,
            initialized: false,
            visible: false,
            texture: None,
            preview_size: [480, 270],
            aspects: BTreeMap::new(),
            frame_pending: false,
            tab: 0,
            selected_layer: None,
            scene_pending: false,
            last_scene: Instant::now(),
            ptz_active: None,
        }
    }
}
impl Camera {
    pub fn has_unsaved_changes(&self) -> bool {
        self.modified || self.config != self.saved_config || self.scene_pending || self.save_pending
    }
    pub fn quit_background(&mut self, ctx: &egui::Context) {
        self.send(ctx, json!({"op":"quit"}));
    }
    pub fn set_direct(&mut self, direct: bool) {
        self.direct = direct;
    }
    pub fn start_background_connection(&mut self, ctx: &egui::Context) {
        if let Err(error) = self.connect(ctx) {
            self.error = Some(error);
        }
    }
    pub fn take_action(&mut self) -> Option<Value> {
        self.actions.pop_front()
    }
    fn preview_path(&self) -> PathBuf {
        if !self.direct {
            return crate::background::runtime().join("preview.rgba");
        }
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })))
            .join(format!(
                "laptop-control-preview-{}.rgba",
                std::process::id()
            ))
    }
    fn connect(&mut self, ctx: &egui::Context) -> Result<(), String> {
        if self.stdin.is_some() {
            return Ok(());
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if !self.direct {
            let stream = crate::background::connect()?;
            let reader = stream.try_clone().map_err(|e| e.to_string())?;
            let (tx, rx) = mpsc::channel();
            let repaint = ctx.clone();
            thread::spawn(move || {
                for line in BufReader::new(reader).lines().map_while(Result::ok) {
                    if let Ok(value) = serde_json::from_str(&line) {
                        if tx.send(value).is_err() {
                            return;
                        }
                        repaint.request_repaint();
                    }
                }
                let _ = tx.send(json!({"backend_disconnected":true}));
                repaint.request_repaint();
            });
            self.stdin = Some(Transport::Socket(stream));
            self.rx = Some(rx);
            writeln!(
                self.stdin.as_mut().unwrap(),
                "{}",
                json!({"op":"subscribe"})
            )
            .map_err(|e| e.to_string())?;
            return Ok(());
        }
        let helper = crate::background::helper();
        let mut child = Command::new("/usr/bin/python3")
            .arg(helper)
            .env(
                "LAPTOP_CONTROL_RASTER_BINARY",
                std::env::current_exe().map_err(|e| e.to_string())?,
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        self.stdin = child.stdin.take().map(Transport::Pipe);
        let output = child.stdout.take().unwrap();
        let errors = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        let out_tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str(&line) {
                    if out_tx.send(v).is_err() {
                        break;
                    }
                    repaint.request_repaint();
                }
            }
        });
        let repaint = ctx.clone();
        thread::spawn(move || {
            for line in BufReader::new(errors).lines().map_while(Result::ok) {
                if tx.send(json!({"diagnostic":line})).is_err() {
                    break;
                }
                repaint.request_repaint();
            }
        });
        self.rx = Some(rx);
        self.child = Some(child);
        Ok(())
    }
    fn send(&mut self, ctx: &egui::Context, command: Value) {
        if let Err(error) = self.connect(ctx).and_then(|()| {
            writeln!(self.stdin.as_mut().unwrap(), "{command}").map_err(|e| e.to_string())
        }) {
            self.error = Some(format!("Camera helper: {error}"));
            self.stdin = None;
            self.state = "Error".into();
            self.running = false;
        }
    }
    pub fn set_visible(&mut self, ctx: &egui::Context, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            if self.initialized {
                self.send(
                    ctx,
                    json!({"op":"preview","enabled":visible,"path":self.preview_path()}),
                );
            }
            if !visible {
                self.texture = None;
            }
        }
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        let messages: Vec<_> = self
            .rx
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for v in messages {
            if let Some(book) = v.get("live_book") {
                if let Ok(mut config) = serde_json::from_value::<CameraConfig>(book.clone()) {
                    config.migrate();
                    self.modified = self.config != config;
                    self.config = config;
                    self.selected_layer = None;
                }
            }
            if v["backend_disconnected"].as_bool() == Some(true) {
                self.stdin = None;
                self.running = false;
                self.save_pending = false;
                self.state = "Disconnected".into();
                self.error =
                    Some("Background connection lost. Rescan cameras to reconnect.".into());
            }
            if let Some(value) = v["virtual_enabled"].as_bool() {
                self.virtual_enabled = value;
            }
            if v["action"].as_str() == Some("selected_scene") {
                if let Some(i) = v["index"]
                    .as_u64()
                    .map(|i| i as usize)
                    .filter(|i| *i < self.config.scenes.len())
                {
                    self.config.selected_scene = i;
                    self.selected_layer = None;
                    self.scene_pending = true;
                }
            } else if v.get("action").is_some() {
                self.actions.push_back(v.clone());
            }
            if let Some(line) = v["diagnostic"].as_str() {
                self.diagnostics.push_back(line.to_owned());
                while self.diagnostics.len() > 12 {
                    self.diagnostics.pop_front();
                }
            }
            if let Some(path) = v["logo"].as_str() {
                let name = std::path::Path::new(path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or("Logo".into());
                self.config.scene_mut().layers.push(Layer {
                    path: path.into(),
                    name,
                    ..Layer::default()
                });
                self.selected_layer = Some(self.config.scene().layers.len() - 1);
                self.scene_pending = true;
            }
            if let Some(state) = v["state"].as_str() {
                self.state = state.into();
            }
            if let Some(running) = v["running"].as_bool() {
                self.running = running;
                if !running && self.state == "Stopped" {
                    self.texture = None;
                }
            }
            if let Some(output) = v["output"].as_str() {
                self.output = output.into();
            }
            if let Some(device) = v["device"].as_str() {
                if !device.is_empty() {
                    if self.config.input != device
                        && std::fs::canonicalize(&self.config.input).ok()
                            != std::fs::canonicalize(device).ok()
                    {
                        self.config.controls.clear();
                    }
                    if self.config.input != device {
                        self.modified = true;
                    }
                    self.config.input = device.into();
                }
            }
            if v.get("error").is_some() {
                self.error = v["error"].as_str().map(str::to_owned);
            }
            if let Some(w) = v.get("warnings") {
                self.warnings = serde_json::from_value(w.clone()).unwrap_or_default();
            }
            if let Some(layers) = v["layers"].as_array() {
                self.aspects = layers
                    .iter()
                    .filter_map(|l| {
                        Some((l["id"].as_str()?.to_owned(), l["aspect"].as_f64()? as f32))
                    })
                    .collect();
            }
            if let Some(c) = v.get("controls") {
                if let Ok(c) = serde_json::from_value(c.clone()) {
                    self.controls = c;
                }
            }
            if let Some(d) = v.get("devices") {
                if let Ok(d) = serde_json::from_value(d.clone()) {
                    self.devices = d;
                }
            }
            if v["request"].as_str() == Some("save_camera") && self.save_pending {
                self.save_pending = false;
                if let Some(error) = v["controls_error"].as_str() {
                    self.error = Some(format!("Could not save camera settings: {error}"));
                } else {
                    self.complete_save(ctx, true);
                }
            }
            if v["request"].as_str() == Some("apply_camera_preset")
                && v["controls_error"].is_null()
                && v.get("controls").is_some()
            {
                self.config.controls = self.restorable_values();
                self.scene_pending = true;
            }
            if v.get("frame").is_some() {
                self.preview_size = [
                    v["width"].as_u64().unwrap_or(480) as usize,
                    v["height"].as_u64().unwrap_or(270) as usize,
                ];
                self.frame_pending = true;
            }
        }
        if let Some(child) = &mut self.child {
            if let Ok(Some(status)) = child.try_wait() {
                self.running = false;
                self.state = "Error".into();
                self.error = Some(format!(
                    "Camera helper stopped ({status}). Use Rescan cameras to reconnect."
                ));
                self.stdin = None;
                self.child = None;
            }
        }
        if self.ptz_active.is_some() && !ctx.input(|i| i.pointer.any_down()) {
            let id = self.ptz_active.take().unwrap();
            self.send(ctx, json!({"op":"set","values":{id:"0"}}));
        }
        if self.visible && self.frame_pending {
            self.frame_pending = false;
            if let Ok(bytes) = fs::read(self.preview_path()) {
                if bytes.len() == self.preview_size[0] * self.preview_size[1] * 4 {
                    let image = egui::ColorImage::from_rgba_unmultiplied(self.preview_size, &bytes);
                    if let Some(texture) = &mut self.texture {
                        texture.set(image, egui::TextureOptions::LINEAR);
                    } else {
                        self.texture = Some(ctx.load_texture(
                            "camera-preview",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
            }
        }
        if self.scene_pending {
            self.modified = true;
        }
        if self.scene_pending && self.last_scene.elapsed() >= Duration::from_millis(75) {
            self.scene_pending = false;
            self.last_scene = Instant::now();
            if self.running {
                self.send(
                    ctx,
                    json!({"op":"scene","config":self.config.payload(),"book":self.config}),
                );
            }
        } else if self.scene_pending {
            ctx.request_repaint_after(Duration::from_millis(75));
        }
    }
    fn restorable_values(&self) -> BTreeMap<String, String> {
        self.controls
            .iter()
            .filter(|c| c.restorable())
            .map(|c| (c.id.clone(), value_text(&c.value)))
            .collect()
    }
    pub fn save(&mut self, ctx: &egui::Context) {
        if self.save_pending {
            return;
        }
        if !self.initialized || self.config.input.is_empty() {
            self.complete_save(ctx, false);
        } else {
            self.save_pending = true;
            self.send(ctx, json!({"op":"read_controls","request":"save_camera"}));
            if self.stdin.is_none() {
                self.save_pending = false;
            }
        }
    }
    fn complete_save(&mut self, ctx: &egui::Context, read_controls: bool) {
        self.modified = true;
        if read_controls {
            self.config.controls = self.restorable_values();
        }
        if std::mem::take(&mut self.scene_pending) && self.running {
            self.send(
                ctx,
                json!({"op":"scene","config":self.config.payload(),"book":self.config}),
            );
        }
        self.config.store_preset();
        match crate::preferences::save(&self.config, "camera.json") {
            Ok(()) => {
                self.modified = false;
                self.saved_config = self.config.clone();
                self.error = None;
                if !self.direct {
                    self.send(ctx, json!({"op":"reload_scenes","book":self.config}));
                }
            }
            Err(e) => self.error = Some(e),
        }
        ctx.request_repaint();
    }
    fn toolbar(&mut self, ui: &mut egui::Ui, demo: bool) {
        let ctx = ui.ctx().clone();
        let busy = self.state == "Starting" || self.state == "Stopping" || self.save_pending;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !demo && !busy && !self.running && !self.config.input.is_empty(),
                    egui::Button::new(if self.state == "Starting" {
                        "Starting…"
                    } else {
                        "Start camera"
                    })
                    .min_size(Vec2::new(132., 34.)),
                )
                .clicked()
            {
                self.state = "Starting".into();
                self.error = None;
                self.send(
                    &ctx,
                    json!({"op":"preview","enabled":true,"path":self.preview_path()}),
                );
                self.send(
                    &ctx,
                    json!({"op":"start","config":self.config.payload(),"book":self.config}),
                );
            }
            if ui
                .add_enabled(
                    !demo && (self.running || self.state == "Starting"),
                    egui::Button::new("Stop camera").min_size(Vec2::new(122., 34.)),
                )
                .clicked()
            {
                self.state = "Stopping".into();
                self.send(&ctx, json!({"op":"stop"}));
            }
            ui.separator();
            let mut virtual_output = self.virtual_enabled;
            if ui
                .add_enabled_ui(!demo, |ui| {
                    widgets::checkbox(ui, &mut virtual_output, "Virtual camera output")
                })
                .inner
                .changed()
            {
                self.send(
                    &ctx,
                    json!({"op":"virtual","enabled":virtual_output,"config":self.config.payload()}),
                );
            }
            if ui
                .add_enabled(!demo && !busy, egui::Button::new("Rescan cameras"))
                .clicked()
            {
                self.send(&ctx, json!({"op":"list","device":self.config.input}));
            }
            let dirty = self.has_unsaved_changes();
            if ui
                .add_enabled(
                    !demo && dirty && !self.save_pending,
                    egui::Button::new(if self.save_pending {
                        "Saving…"
                    } else if dirty {
                        "Save changes"
                    } else {
                        "Saved"
                    }),
                )
                .clicked()
            {
                self.save(&ctx);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (color, label) = match self.state.as_str() {
                    "Live" => (Color32::from_rgb(55, 165, 110), "CAMERA LIVE"),
                    "Starting" => (Color32::from_rgb(205, 150, 55), "STARTING..."),
                    "Stopping" => (Color32::from_rgb(205, 150, 55), "STOPPING..."),
                    "Error" => (Color32::from_rgb(215, 90, 80), "ERROR | not streaming"),
                    _ => (ui.visuals().weak_text_color(), "CAMERA OFF"),
                };
                ui.label(RichText::new(label).color(color).strong());
                ui.label(if self.virtual_enabled {
                    "OUTPUT ON"
                } else {
                    "OUTPUT OFF"
                });
            });
        });
        ui.add_space(6.);
        if let Some(error) = &self.error {
            ui.colored_label(Color32::from_rgb(215, 100, 75), error);
        }
        if !self.warnings.is_empty() {
            egui::CollapsingHeader::new(format!("{} camera setting notices", self.warnings.len()))
                .show(ui, |ui| {
                    for warning in &self.warnings {
                        ui.small(warning);
                    }
                });
        }
        if !self.diagnostics.is_empty() {
            egui::CollapsingHeader::new("Camera diagnostics").show(ui, |ui| {
                for line in &self.diagnostics {
                    ui.small(line);
                }
            });
        }
    }
    fn preview(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        let height = (ui.available_height() * 0.34).clamp(160., 280.);
        let (outer, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
        ui.painter().rect_filled(outer, 12., Color32::from_gray(14));
        let aspect = self.config.width as f32 / self.config.height as f32;
        let image_size = if width / height > aspect {
            Vec2::new(height * aspect, height)
        } else {
            Vec2::new(width, width / aspect)
        };
        let rect = egui::Rect::from_center_size(outer.center(), image_size);
        if let Some(texture) = &self.texture {
            ui.painter().image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.)),
                Color32::WHITE,
            );
        } else {
            let text = match self.state.as_str() {
                "Starting" => "Opening camera…",
                "Error" => "Camera could not start · see the message above",
                _ => {
                    if self.virtual_enabled {
                        "Camera off · virtual output is on standby"
                    } else {
                        "Camera is off · press Start camera"
                    }
                }
            };
            ui.painter().text(
                outer.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(18.),
                Color32::from_gray(185),
            );
        }
        if self.tab == 1 && self.running && self.texture.is_some() {
            for (i, layer) in self.config.scene_mut().layers.iter_mut().enumerate() {
                if !layer.enabled {
                    continue;
                }
                let wf = layer.width;
                let hf = (wf
                    * aspect
                    * self.aspects.get(&layer.id).copied().unwrap_or(
                        if layer.kind == LayerKind::Text {
                            0.15
                        } else {
                            1.
                        },
                    ))
                .min(1.);
                let layer_rect = egui::Rect::from_min_size(
                    rect.min
                        + Vec2::new(
                            layer.x * (1. - wf) * rect.width(),
                            layer.y * (1. - hf) * rect.height(),
                        ),
                    Vec2::new(wf * rect.width(), hf * rect.height()),
                );
                let move_response = ui.interact(
                    layer_rect,
                    ui.id().with(("move-layer", &layer.id)),
                    egui::Sense::click_and_drag(),
                );
                if move_response.clicked() || move_response.drag_started() {
                    self.selected_layer = Some(i);
                }
                if self.selected_layer == Some(i) {
                    ui.painter().rect_stroke(
                        layer_rect,
                        3.,
                        egui::Stroke::new(1.5, Color32::from_rgb(165, 135, 250)),
                        egui::StrokeKind::Inside,
                    );
                    let handle =
                        egui::Rect::from_center_size(layer_rect.right_bottom(), Vec2::splat(14.));
                    let resize = ui.interact(
                        handle,
                        ui.id().with(("resize-layer", &layer.id)),
                        egui::Sense::drag(),
                    );
                    ui.painter()
                        .rect_filled(handle.shrink(3.), 2., Color32::WHITE);
                    if resize.dragged() {
                        layer.width =
                            (layer.width + resize.drag_delta().x / rect.width()).clamp(0.02, 1.);
                        self.scene_pending = true;
                    } else if move_response.dragged() {
                        let delta = move_response.drag_delta();
                        layer.x =
                            (layer.x + delta.x / ((1. - wf) * rect.width()).max(1.)).clamp(0., 1.);
                        layer.y =
                            (layer.y + delta.y / ((1. - hf) * rect.height()).max(1.)).clamp(0., 1.);
                        self.scene_pending = true;
                    }
                }
            }
        }
        ui.horizontal(|ui| {
            ui.weak(format!(
                "{} × {}  ·  {} fps output and preview  ·  {}",
                self.config.width,
                self.config.height,
                self.config.fps,
                self.config.scene().name
            ));
            if self.running {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.small(format!("Laptop Control Camera · {}", self.output));
                });
            }
        });
    }
    fn capture(&mut self, ui: &mut egui::Ui, demo: bool) {
        let before = (
            self.config.input.clone(),
            self.config.width,
            self.config.height,
            self.config.fps,
            self.config.format.clone(),
            self.config.mirror,
        );
        let ctx = ui.ctx().clone();
        ui.columns(2,|columns|{
            self.preset_editor(&mut columns[0], demo);
            columns[0].add_space(12.);
            widgets::section(&mut columns[0],"Capture settings",|ui|{
                ui.add_enabled_ui(!self.running&&!self.virtual_enabled&&self.state!="Starting",|ui|{
                    let old=self.config.input.clone();
                    widgets::field(ui,"Camera",|ui|{egui::ComboBox::from_id_salt("device").selected_text(self.devices.iter().find(|d|d.path==self.config.input).map(|d|d.name.as_str()).unwrap_or("Select camera")).width(ui.available_width()).show_ui(ui,|ui|{for d in &self.devices{ui.selectable_value(&mut self.config.input,d.path.clone(),&d.name);}});});
                    if old!=self.config.input&&!demo{self.config.controls.clear();self.send(&ctx,json!({"op":"list","device":self.config.input}));}
                    widgets::field(ui,"Resolution",|ui|{let mut size=(self.config.width,self.config.height);egui::ComboBox::from_id_salt("resolution").width(ui.available_width()).selected_text(format!("{} × {}",size.0,size.1)).show_ui(ui,|ui|{for s in [(640,360),(640,480),(1280,720),(1920,1080)]{ui.selectable_value(&mut size,s,format!("{} × {}",s.0,s.1));}});self.config.width=size.0;self.config.height=size.1;});
                    widgets::field(ui,"Frame rate",|ui|{egui::ComboBox::from_id_salt("fps").width(ui.available_width()).selected_text(format!("{} fps",self.config.fps)).show_ui(ui,|ui|{for fps in [10,15,20,30]{ui.selectable_value(&mut self.config.fps,fps,format!("{fps} fps"));}});});
                    widgets::field(ui,"Format",|ui|{egui::ComboBox::from_id_salt("format").width(ui.available_width()).selected_text(&self.config.format).show_ui(ui,|ui|{ui.selectable_value(&mut self.config.format,"MJPG".into(),"MJPEG · Intel decoding");ui.selectable_value(&mut self.config.format,"YUYV".into(),"YUYV · uncompressed");});});
                    widgets::field(ui,"Mirror",|ui|{widgets::checkbox(ui,&mut self.config.mirror,"Flip camera horizontally");});
                });
                ui.add_space(10.);ui.weak("Stop camera and disable virtual output to change capture format. Preview uses the selected frame rate.");
                ui.separator();ui.label("Meeting setup");ui.label("1. Enable Virtual camera output.");ui.label("2. Press Start camera and wait for CAMERA LIVE.");ui.label("3. Choose Laptop Control Camera in Teams; refresh its camera list if needed.");
            });
            widgets::section(&mut columns[1],"Camera controls",|ui|{self.hardware_controls(ui);});
        });
        if before
            != (
                self.config.input.clone(),
                self.config.width,
                self.config.height,
                self.config.fps,
                self.config.format.clone(),
                self.config.mirror,
            )
        {
            self.modified = true;
        }
    }
    fn preset_editor(&mut self, ui: &mut egui::Ui, demo: bool) {
        widgets::section(ui, "Camera presets", |ui| {
            ui.weak("Save lighting, exposure and color settings without stopping the live camera.");
            let previous = self.config.selected_preset;
            widgets::field(ui, "Preset", |ui| {
                egui::ComboBox::from_id_salt("camera-preset")
                    .width(ui.available_width())
                    .selected_text(
                        self.config
                            .selected_preset
                            .and_then(|i| self.config.presets.get(i))
                            .map(|p| p.name.as_str())
                            .unwrap_or("No preset"),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.config.selected_preset, None, "No preset");
                        for (i, preset) in self.config.presets.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.config.selected_preset,
                                Some(i),
                                &preset.name,
                            );
                        }
                    });
            });
            if previous != self.config.selected_preset {
                self.modified = true;
            }
            if let Some(i) = self.config.selected_preset {
                widgets::field(ui, "Preset name", |ui| {
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.config.presets[i].name)
                                .desired_width(ui.available_width()),
                        )
                        .changed()
                    {
                        self.modified = true;
                    }
                });
            }
            ui.add_enabled_ui(!demo && !self.save_pending, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("New preset").clicked() {
                        self.config.presets.push(CameraPreset {
                            name: format!("Preset {}", self.config.presets.len() + 1),
                            input: self.config.input.clone(),
                            controls: self.restorable_values(),
                        });
                        self.config.selected_preset = Some(self.config.presets.len() - 1);
                        self.modified = true;
                    }
                    if let Some(i) = self.config.selected_preset {
                        if ui.button("Load preset").clicked() {
                            let preset = &self.config.presets[i];
                            let same_device = preset.input == self.config.input || matches!(
                                (fs::canonicalize(&preset.input), fs::canonicalize(&self.config.input)),
                                (Ok(a), Ok(b)) if a == b
                            );
                            if same_device {
                                self.send(ui.ctx(), json!({"op":"apply_preset","values":preset.controls,"request":"apply_camera_preset"}));
                                self.modified = true;
                            } else {
                                self.error = Some("Select the camera this preset was saved for before loading it.".into());
                            }
                        }
                        if ui.button("Save preset").clicked() { self.save(ui.ctx()); }
                        if ui.button("Delete preset").clicked() {
                            self.config.presets.remove(i);
                            self.config.selected_preset = None;
                            self.modified = true;
                        }
                    }
                });
            });
        });
    }
    fn hardware_controls(&mut self, ui: &mut egui::Ui) {
        let mut changes = Vec::new();
        let mut group = String::new();
        for c in &mut self.controls {
            if [
                "pixelformat",
                "resolution",
                "fps",
                "path",
                "real_path",
                "card",
                "driver",
            ]
            .contains(&c.id.as_str())
            {
                continue;
            }
            if group != c.group {
                group = c.group.clone();
                ui.add_space(8.);
                ui.label(RichText::new(&group).strong());
                ui.separator();
            }
            ui.push_id(&c.id, |ui| {
                ui.add_enabled_ui(
                    !(c.inactive || c.readonly || self.running && c.reopener),
                    |ui| {
                        if c.zeroer {
                            ui.label(&c.name);
                            ui.horizontal(|ui| {
                                for (label, value) in [
                                    ("Hold −", c.min.unwrap_or(-1.)),
                                    ("Hold +", c.max.unwrap_or(1.)),
                                ] {
                                    let r = ui.button(label);
                                    if r.is_pointer_button_down_on()
                                        && self.ptz_active.as_deref() != Some(&c.id)
                                    {
                                        self.ptz_active = Some(c.id.clone());
                                        changes.push((c.id.clone(), value.to_string()));
                                    }
                                }
                            });
                        } else {
                            match c.kind.as_str() {
                                "integer" => {
                                    let mut value = c.value.as_f64().unwrap_or(0.);
                                    let r = widgets::slider(
                                        ui,
                                        &c.name,
                                        &mut value,
                                        c.min.unwrap_or(0.)..=c.max.unwrap_or(100.),
                                        c.step.unwrap_or(1.),
                                        "",
                                    );
                                    if r.changed() {
                                        c.value = json!(value);
                                        if !r.dragged() || r.drag_stopped() {
                                            changes.push((c.id.clone(), value.to_string()));
                                        }
                                    }
                                }
                                "boolean" => {
                                    let mut value = c
                                        .value
                                        .as_bool()
                                        .unwrap_or_else(|| c.value.as_i64().unwrap_or(0) != 0);
                                    if widgets::checkbox(ui, &mut value, &c.name).changed() {
                                        c.value = json!(i32::from(value));
                                        changes.push((c.id.clone(), i32::from(value).to_string()));
                                    }
                                }
                                "menu" => {
                                    widgets::field(ui, &c.name, |ui| {
                                        let mut value = value_text(&c.value);
                                        let old = value.clone();
                                        egui::ComboBox::from_id_salt("value")
                                            .width(ui.available_width())
                                            .selected_text(
                                                c.menu
                                                    .iter()
                                                    .find(|m| m.id == value)
                                                    .map(|m| m.name.as_str())
                                                    .unwrap_or(&value),
                                            )
                                            .show_ui(ui, |ui| {
                                                for m in &c.menu {
                                                    ui.selectable_value(
                                                        &mut value,
                                                        m.id.clone(),
                                                        &m.name,
                                                    );
                                                }
                                            });
                                        if value != old {
                                            c.value = json!(value);
                                            changes.push((c.id.clone(), value));
                                        }
                                    });
                                }
                                "button" => {
                                    ui.label(&c.name);
                                    ui.horizontal_wrapped(|ui| {
                                        for choice in &c.menu {
                                            if ui.button(&choice.name).clicked() {
                                                changes.push((c.id.clone(), choice.id.clone()));
                                            }
                                        }
                                    });
                                }
                                _ => {
                                    ui.label(format!("{}: {}", c.name, value_text(&c.value)));
                                }
                            }
                        }
                        if c.inactive {
                            ui.small("Controlled automatically by the camera");
                        }
                        if !c.default.is_null()
                            && matches!(c.kind.as_str(), "integer" | "menu" | "boolean")
                        {
                            ui.horizontal(|ui| {
                                if ui
                                    .small_button("Reset to default")
                                    .on_hover_text(&c.help)
                                    .clicked()
                                {
                                    changes.push((c.id.clone(), value_text(&c.default)));
                                }
                            });
                        }
                        ui.add_space(8.);
                    },
                );
            });
        }
        if !changes.is_empty() {
            self.modified = true;
            for (id, value) in &changes {
                if self.controls.iter().any(|c| c.id == *id && c.restorable()) {
                    self.config.controls.insert(id.clone(), value.clone());
                }
            }
            let values: BTreeMap<_, _> = changes.into_iter().collect();
            self.send(ui.ctx(), json!({"op":"set","values":values}));
        }
    }
    fn scene_list(&mut self, ui: &mut egui::Ui) {
        widgets::section(ui, "Scenes", |ui| {
            egui::ScrollArea::vertical()
                .id_salt("scene-list")
                .max_height(116.)
                .show_rows(ui, 32., self.config.scenes.len(), |ui, range| {
                    for i in range {
                        if ui
                            .selectable_label(
                                self.config.selected_scene == i,
                                &self.config.scenes[i].name,
                            )
                            .clicked()
                        {
                            self.config.selected_scene = i;
                            self.selected_layer = None;
                            self.scene_pending = true;
                        }
                    }
                });
            ui.horizontal(|ui| {
                if ui.button("+ Scene").clicked() {
                    self.config.scenes.push(Scene {
                        name: format!("Scene {}", self.config.scenes.len() + 1),
                        ..Scene::default()
                    });
                    self.config.selected_scene = self.config.scenes.len() - 1;
                    self.selected_layer = None;
                    self.scene_pending = true;
                }
                if ui.button("Duplicate").clicked() {
                    self.config.duplicate();
                    self.selected_layer = None;
                    self.scene_pending = true;
                }
                if ui
                    .add_enabled(self.config.scenes.len() > 1, egui::Button::new("Delete"))
                    .clicked()
                {
                    self.config.scenes.remove(self.config.selected_scene);
                    self.config.selected_scene =
                        self.config.selected_scene.min(self.config.scenes.len() - 1);
                    self.selected_layer = None;
                    self.scene_pending = true;
                }
            });
            widgets::field(ui, "Scene name", |ui| {
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.config.scene_mut().name)
                            .desired_width(ui.available_width())
                            .margin(egui::Margin::symmetric(10, 8)),
                    )
                    .changed()
                {
                    self.modified = true;
                }
            });
        });
    }
    fn layer_list(&mut self, ui: &mut egui::Ui, demo: bool) {
        widgets::section(ui, "Layers", |ui| {
            let count = self.config.scene().layers.len();
            egui::ScrollArea::vertical()
                .id_salt("layer-list")
                .max_height(180.)
                .show_rows(ui, 32., count, |ui, range| {
                    for row in range {
                        let i = count - 1 - row;
                        let layer = &mut self.config.scenes[self.config.selected_scene].layers[i];
                        ui.push_id(&layer.id, |ui| {
                            ui.horizontal(|ui| {
                                if widgets::checkbox(ui, &mut layer.enabled, "")
                                    .on_hover_text("Layer visibility")
                                    .changed()
                                {
                                    self.scene_pending = true;
                                }
                                if ui
                                    .selectable_label(self.selected_layer == Some(i), &layer.name)
                                    .clicked()
                                {
                                    self.selected_layer = Some(i);
                                }
                            });
                        });
                    }
                });
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!demo, egui::Button::new("+ SVG / PNG logo…"))
                    .clicked()
                {
                    self.send(ui.ctx(), json!({"op":"pick_logo"}));
                }
                if ui.button("+ Text").clicked() {
                    self.config.scene_mut().layers.push(Layer::text());
                    self.selected_layer = Some(self.config.scene().layers.len() - 1);
                    self.scene_pending = true;
                }
            });
            if let Some(i) = self.selected_layer.filter(|i| *i < count) {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(i + 1 < count, egui::Button::new("Raise"))
                        .clicked()
                    {
                        self.config.scene_mut().layers.swap(i, i + 1);
                        self.selected_layer = Some(i + 1);
                        self.scene_pending = true;
                    }
                    if ui.add_enabled(i > 0, egui::Button::new("Lower")).clicked() {
                        self.config.scene_mut().layers.swap(i, i - 1);
                        self.selected_layer = Some(i - 1);
                        self.scene_pending = true;
                    }
                    if ui.button("Remove").clicked() {
                        self.config.scene_mut().layers.remove(i);
                        self.selected_layer = None;
                        self.scene_pending = true;
                    }
                });
            }
            ui.weak(
                "Top layers appear in front. Drag or resize the selected layer in the preview.",
            );
        });
    }
    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some(i) = self
            .selected_layer
            .filter(|i| *i < self.config.scene().layers.len())
        else {
            ui.weak("Select a layer to edit its properties.");
            return;
        };
        let layer = &mut self.config.scenes[self.config.selected_scene].layers[i];
        let before = layer.clone();
        widgets::field(ui, "Name", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut layer.name)
                    .desired_width(ui.available_width())
                    .margin(egui::Margin::symmetric(10, 8)),
            );
        });
        if layer.kind == LayerKind::Image {
            widgets::field(ui, "Image path", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut layer.path)
                        .desired_width(ui.available_width())
                        .margin(egui::Margin::symmetric(10, 8)),
                );
            });
        } else {
            ui.label("Text");
            ui.add(
                egui::TextEdit::multiline(&mut layer.text)
                    .desired_width(ui.available_width())
                    .desired_rows(2),
            );
            widgets::field(ui, "Font", |ui| {
                egui::ComboBox::from_id_salt("font")
                    .width(ui.available_width())
                    .selected_text(&layer.font)
                    .show_ui(ui, |ui| {
                        for font in ["Sans", "Serif", "Monospace"] {
                            ui.selectable_value(&mut layer.font, font.into(), font);
                        }
                    });
            });
            widgets::slider(ui, "Font size", &mut layer.font_size, 8. ..=144., 1., " px");
            ui.horizontal(|ui| {
                widgets::checkbox(ui, &mut layer.bold, "Bold");
                ui.label("Text color");
                ui.color_edit_button_srgba_unmultiplied(&mut layer.color);
            });
        }
        widgets::percent(ui, "Horizontal position", &mut layer.x);
        widgets::percent(ui, "Vertical position", &mut layer.y);
        widgets::percent(ui, "Width", &mut layer.width);
        widgets::percent(ui, "Opacity", &mut layer.opacity);
        if *layer != before {
            self.scene_pending = true;
        }
    }
    fn scenes(&mut self, ui: &mut egui::Ui, demo: bool) {
        let width = ui.available_width();
        let height = ui.available_height().max(180.);
        let left_width = (width * 0.33).clamp(300., 800.);
        let gap = ui.spacing().item_spacing.x;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(left_width, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("scene-docks")
                        .max_height(height)
                        .show(ui, |ui| {
                            self.scene_list(ui);
                            ui.add_space(10.);
                            self.layer_list(ui, demo);
                        });
                },
            );
            ui.allocate_ui_with_layout(
                Vec2::new((width - left_width - gap).max(200.), height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    widgets::section(ui, "Layer inspector", |ui| {
                        ui.set_min_height((height - 36.).max(100.));
                        egui::ScrollArea::vertical()
                            .id_salt("layer-inspector")
                            .max_height((height - 80.).max(80.))
                            .show(ui, |ui| self.inspector(ui));
                    });
                },
            );
        });
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, demo: bool) {
        if !self.initialized {
            self.initialized = true;
            if !demo {
                self.send(ui.ctx(), json!({"op":"list","device":self.config.input}));
                self.send(
                    ui.ctx(),
                    json!({"op":"preview","enabled":true,"path":self.preview_path()}),
                );
            }
        }
        ui.heading(RichText::new("Webcam studio").size(26.));
        ui.add_space(8.);
        self.toolbar(ui, demo);
        ui.add_enabled_ui(!self.save_pending, |ui| {
            ui.add_space(8.);
            self.preview(ui);
            ui.add_space(8.);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, 0, "Capture");
                ui.selectable_value(&mut self.tab, 1, "Scenes & Logos");
            });
            ui.add_space(12.);
            if self.tab == 0 {
                egui::ScrollArea::vertical()
                    .id_salt("webcam-fields")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.capture(ui, demo));
            } else {
                self.scenes(ui, demo);
            }
        });
        if self.scene_pending {
            self.modified = true;
        }
    }
    pub fn preview_ready(&self) -> bool {
        self.running && self.texture.is_some()
    }
    pub fn demonstration(&mut self, ctx: &egui::Context) {
        self.direct = true;
        self.state = "Starting".into();
        self.config = CameraConfig::default();
        self.config.width = 640;
        self.config.height = 360;
        self.config.presets = vec![
            CameraPreset {
                name: "Office".into(),
                ..CameraPreset::default()
            },
            CameraPreset {
                name: "Backlight".into(),
                ..CameraPreset::default()
            },
        ];
        self.config.selected_preset = Some(0);
        self.config.scene_mut().name = "Meeting".into();
        self.config.scene_mut().layers.push(Layer {
            path: format!("{}/assets/laptop-control.png", env!("CARGO_MANIFEST_DIR")),
            x: 1.,
            y: 0.,
            width: 0.16,
            ..Layer::default()
        });
        self.config.scene_mut().layers.push(Layer {
            text: "Laptop Control · Live studio".into(),
            font_size: 22.,
            width: 0.8,
            y: 0.9,
            ..Layer::text()
        });
        self.config.scenes.push(Scene {
            name: "Presentation".into(),
            ..Scene::default()
        });
        self.selected_layer = Some(1);
        self.tab = 1;
        self.visible = true;
        self.initialized = true;
        self.send(
            ctx,
            json!({"op":"preview","enabled":true,"path":self.preview_path()}),
        );
        self.send(
            ctx,
            json!({"op":"start","config":self.config.payload(),"test":true}),
        );
    }
    pub fn close(&mut self) {
        if let Some(mut writer) = self.stdin.take() {
            if self.direct {
                let _ = writeln!(writer, "{}", json!({"op":"quit"}));
            } else {
                let _ = writeln!(writer, "{}", json!({"op":"preview","enabled":false}));
            }
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if self.direct {
            let _ = fs::remove_file(self.preview_path());
            let _ = fs::remove_file(self.preview_path().with_extension("rgba.new"));
        }
    }
}
impl Drop for Camera {
    fn drop(&mut self) {
        self.close();
    }
}
