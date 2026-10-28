mod background;
mod blur;
mod camera;
mod camera_model;
mod navigation;
mod preferences;
mod system_info;
mod widgets;
use navigation::Page;
mod chart;
mod profile_buttons;
mod theme;
use eframe::egui::{self, Color32, RichText, Vec2};
use raf_fans::*;
use std::{
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
struct Work {
    request: Request,
    command: bool,
}
struct ResultMessage {
    result: Result<Status, String>,
    command: bool,
}
struct App {
    status: Status,
    saved: Profiles,
    drafts: [Config; 2],
    selected: Profile,
    page: Page,
    settings_section: usize,
    preferences: preferences::Preferences,
    saved_preferences: preferences::Preferences,
    info: system_info::SystemInfo,
    camera: camera::Camera,
    settings_draft: Settings,
    error: Option<String>,
    tx: mpsc::Sender<Work>,
    rx: mpsc::Receiver<ResultMessage>,
    last: Instant,
    pending: usize,
    command_pending: usize,
    demo: bool,
    screenshot: Option<String>,
    capture_sent: bool,
    require_camera_preview: bool,
    started: Instant,
    initialized: bool,
    connected: bool,
    selected_point: [usize; 2],
    kde_source: String,
    theme_checked: Instant,
    save_rect: egui::Rect,
    blur: Option<blur::Blur>,
    close_dialog: bool,
    close_after_save: bool,
    allow_close: bool,
    quit_background: bool,
}
impl App {
    fn new(
        ctx: &egui::Context,
        socket: String,
        demo: bool,
        screenshot: Option<String>,
        theme: theme::Theme,
    ) -> Self {
        let (tx, commands) = mpsc::channel::<Work>();
        let (results, rx) = mpsc::channel();
        let repaint = ctx.clone();
        thread::spawn(move || {
            while let Ok(work) = commands.recv() {
                let result = exchange(&socket, work.request);
                if results
                    .send(ResultMessage {
                        result,
                        command: work.command,
                    })
                    .is_err()
                {
                    break;
                }
                repaint.request_repaint();
            }
        });
        let mut saved = Profiles::default();
        if demo {
            saved.selected = Profile::Office;
        }
        let status = if demo {
            Status {
                config: saved.active(),
                profiles: saved.clone(),
                temperatures: Temperatures {
                    cpu: Some(62.),
                    gpu: None,
                    gpu_smi: None,
                    gpu_usage: None,
                    gpu_expected: false,
                    gpu_state: "D3cold · sleeping".into(),
                },
                requested: Some([46, 0]),
                control_temperatures: [Some(62.), None],
                raw_fans: [None, None],
                message: "Simulated temperature readings".into(),
                simulated: true,
                sample_age_ms: 0,
            }
        } else {
            Status::default()
        };
        let mut preferences = if demo {
            preferences::Preferences::default()
        } else {
            preferences::load()
        };
        preferences.appearance.theme = theme;
        theme::apply_options(ctx, &preferences.appearance, &theme::kde_source());
        let isolated = demo || screenshot.is_some();
        let mut camera = camera::Camera::default();
        camera.set_direct(isolated);
        if !isolated {
            camera.start_background_connection(ctx);
        }
        Self {
            status,
            drafts: [saved.office.clone(), saved.gaming.clone()],
            selected: saved.selected,
            page: Page::Fans,
            settings_section: 0,
            saved_preferences: preferences.clone(),
            preferences,
            info: system_info::SystemInfo::new(),
            camera,
            settings_draft: saved.settings.clone(),
            saved,
            error: None,
            tx,
            rx,
            last: Instant::now() - Duration::from_secs(3),
            pending: 0,
            command_pending: 0,
            demo,
            screenshot,
            capture_sent: false,
            require_camera_preview: false,
            started: Instant::now(),
            initialized: demo,
            connected: demo,
            selected_point: [0, 0],
            kde_source: theme::kde_source(),
            theme_checked: Instant::now(),
            save_rect: egui::Rect::NOTHING,
            blur: None,
            close_dialog: false,
            close_after_save: false,
            allow_close: false,
            quit_background: false,
        }
    }
    fn dirty(&self) -> bool {
        if self.page == Page::Settings {
            return self.settings_draft != self.saved.settings
                || self.preferences != self.saved_preferences;
        }
        self.selected
            .index()
            .is_some_and(|i| self.drafts[i] != self.saved.config(self.selected))
    }
    fn finish_close(&mut self, ctx: &egui::Context) {
        self.allow_close = true;
        self.close_dialog = false;
        if self.quit_background {
            self.camera.quit_background(ctx);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    fn request_close(&mut self, ctx: &egui::Context, quit_background: bool) {
        self.quit_background |= quit_background;
        if self.camera.has_unsaved_changes() {
            self.close_dialog = true;
        } else {
            self.finish_close(ctx);
        }
    }
    fn camera_close_dialog(&mut self, ctx: &egui::Context) {
        if !self.close_dialog {
            return;
        }
        egui::Modal::new(egui::Id::new("save-camera-before-closing")).show(ctx, |ui| {
            ui.set_width(430.);
            ui.heading("Save camera changes before closing?");
            ui.label("Your camera settings, presets and scene edits have unsaved changes.");
            if let Some(error) = &self.camera.error {
                ui.colored_label(Color32::from_rgb(215, 100, 75), error);
            }
            ui.add_space(12.);
            ui.add_enabled_ui(!self.camera.save_pending, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Save and close").clicked() {
                        self.camera.save(ctx);
                        self.close_after_save = true;
                    }
                    if ui.button("Close without saving").clicked() {
                        self.finish_close(ctx);
                    }
                    if ui.button("Cancel").clicked() {
                        self.cancel_close();
                    }
                });
            });
            if self.camera.save_pending {
                ui.label("Reading and saving camera settings…");
            }
        });
    }
    fn cancel_close(&mut self) {
        self.close_dialog = false;
        self.close_after_save = false;
        self.quit_background = false;
    }
    fn update_close_after_save(&mut self, ctx: &egui::Context) {
        if self.close_after_save && !self.camera.save_pending {
            self.close_after_save = false;
            if !self.camera.has_unsaved_changes() {
                self.finish_close(ctx);
            }
        }
    }
    fn send(&mut self, request: Request) {
        self.pending += 1;
        self.command_pending += 1;
        let _ = self.tx.send(Work {
            request,
            command: true,
        });
    }
    fn select(&mut self, profile: Profile) {
        self.selected = profile;
        self.page = Page::Fans;
        self.selected_point = [0, 0];
        if self.demo {
            self.saved.selected = profile;
            self.refresh_demo();
        } else {
            self.send(Request::SelectProfile(profile));
        }
    }
    fn save_profile(&mut self) {
        if self.page == Page::Settings {
            self.settings_draft.gpu_usage_in_gaming &= self.preferences.cards.usage;
            if !self.demo {
                if let Err(e) = preferences::save(&self.preferences, "settings.json") {
                    self.error = Some(e);
                    return;
                }
            }
            self.saved_preferences = self.preferences.clone();
            if self.demo {
                self.saved.settings = self.settings_draft.clone();
                self.refresh_demo();
            } else {
                if self.settings_draft != self.saved.settings {
                    self.send(Request::SaveSettings(self.settings_draft.clone()));
                }
            }
            return;
        }
        if let Some(i) = self.selected.index() {
            let config = self.drafts[i].clone();
            if self.demo {
                let _ = self.saved.save_profile(self.selected, config);
                self.refresh_demo();
            } else {
                self.send(Request::SaveProfile {
                    profile: self.selected,
                    config,
                });
            }
        }
    }
    fn refresh_demo(&mut self) {
        self.status.profiles = self.saved.clone();
        self.status.config = self.saved.active();
        self.status.requested = target(&self.status.config, &self.status.temperatures)
            .map(|v| v.map(hardware::effective_percent));
    }
    fn receive(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            if message.command {
                self.command_pending = self.command_pending.saturating_sub(1);
            }
            match message.result {
                Ok(s) => {
                    for (i, profile) in [Profile::Office, Profile::Gaming].into_iter().enumerate() {
                        // Polling refreshes clean drafts; never overwrite unsaved curve edits.
                        if !self.initialized || self.drafts[i] == self.saved.config(profile) {
                            self.drafts[i] = s.profiles.config(profile);
                        }
                    }
                    if !self.initialized || self.settings_draft == self.saved.settings {
                        self.settings_draft = s.profiles.settings.clone();
                    }
                    self.saved = s.profiles.clone();
                    if self.command_pending == 0 {
                        self.selected = self.saved.selected;
                    }
                    self.initialized = true;
                    self.connected = true;
                    self.error = None;
                    self.status = s;
                }
                Err(e) => {
                    self.error = Some(e);
                    if !message.command {
                        self.connected = false;
                    }
                    if !self.initialized {
                        self.status.temperatures = read_temperatures(Path::new("/sys/class/hwmon"));
                    }
                }
            }
        }
    }
    fn capture(&mut self, ctx: &egui::Context) {
        if let Some(path) = &self.screenshot {
            for event in ctx.input(|i| i.events.clone()) {
                if let egui::Event::Screenshot { image, .. } = event {
                    let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                    image::save_buffer(
                        path,
                        &bytes,
                        image.size[0] as u32,
                        image.size[1] as u32,
                        image::ColorType::Rgba8,
                    )
                    .expect("Save preview");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            if !self.capture_sent
                && self.started.elapsed() > Duration::from_secs(2)
                && (!self.require_camera_preview
                    || self.camera.preview_ready()
                    || self.started.elapsed() > Duration::from_secs(12))
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                self.capture_sent = true;
            }
        }
    }
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let collapsed = self.preferences.sidebar_collapsed;
        ui.horizontal(|ui| {
            if !collapsed {
                ui.label(RichText::new("LAPTOP CONTROL").strong());
            }
            if navigation::collapse_button(ui, collapsed)
                .on_hover_text(if collapsed {
                    "Expand sidebar"
                } else {
                    "Collapse sidebar"
                })
                .clicked()
            {
                self.preferences.sidebar_collapsed = !collapsed;
                self.saved_preferences.sidebar_collapsed = !collapsed;
                if !self.demo {
                    if let Err(e) = preferences::save(&self.saved_preferences, "settings.json") {
                        self.error = Some(e);
                    }
                }
            }
        });
        ui.add_space(18.);
        for page in [Page::Fans, Page::Camera, Page::Monitors] {
            if navigation::button(ui, page, self.page == page).clicked() {
                self.page = page;
            }
            ui.add_space(6.);
        }
        ui.add_space((ui.available_height() - 60.).max(0.));
        if navigation::button(ui, Page::Settings, self.page == Page::Settings).clicked() {
            self.page = Page::Settings;
        }
    }
    fn monitors(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("System monitors").size(26.));
        ui.weak("Live system information · optional monitoring uses your saved preferences");
        ui.add_space(20.);
        ui.columns(3, |columns| {
            widgets::section(&mut columns[0], "System", |ui| {
                ui.set_min_height(180.);
                ui.label(&self.info.model);
                ui.separator();
                ui.label(&self.info.cpu);
                ui.label(&self.info.memory);
                ui.label(&self.info.os);
            });
            widgets::section(&mut columns[1], "Power", |ui| {
                ui.set_min_height(180.);
                ui.label(
                    RichText::new(if self.preferences.cards.power {
                        self.info.power.as_str()
                    } else {
                        "Power sampling disabled"
                    })
                    .size(22.),
                );
                ui.label(&self.info.mode);
                ui.weak("Battery power flow; not total wall consumption.");
            });
            widgets::section(&mut columns[2], "Processor usage", |ui| {
                ui.set_min_height(180.);
                ui.label(
                    RichText::new(
                        self.info
                            .cpu_usage
                            .map(|v| format!("CPU {v:.0}%"))
                            .unwrap_or("CPU sampling…".into()),
                    )
                    .size(22.),
                );
                if self.saved.selected == Profile::Gaming {
                    ui.label(
                        self.status
                            .temperatures
                            .gpu_usage
                            .map(|v| format!("dGPU {v:.0}%"))
                            .unwrap_or_else(|| {
                                format!("dGPU {}", self.status.temperatures.gpu_state)
                            }),
                    );
                } else {
                    ui.weak("dGPU usage is available in Gaming with opt-in telemetry.");
                }
            });
        });
        ui.add_space(20.);
        ui.label("Monitoring is paused when this page is hidden. Configure CPU and power sampling in Settings → System monitors.");
        if ui.button("Configure monitoring").clicked() {
            self.page = Page::Settings;
            self.settings_section = 2;
        }
    }
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading(RichText::new("Laptop Control").size(28.));
                ui.weak("InfinityBook Pro Gen7 MK2");
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                ui.label(
                    RichText::new(if self.demo {
                        "PREVIEW"
                    } else if self.connected {
                        "CONNECTED"
                    } else {
                        "RECONNECTING"
                    })
                    .color(ui.visuals().selection.bg_fill),
                );
                if self.camera.running {
                    ui.label(RichText::new("CAMERA LIVE").color(Color32::from_rgb(240, 150, 100)));
                }
            });
        });
        if self.page == Page::Fans {
            ui.horizontal(|ui| {
                ui.weak(if self.selected == Profile::Firmware {
                    "Firmware controls both fans."
                } else {
                    "Edit curves and save this profile."
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(
                        self.command_pending == 0 && (self.connected || self.demo),
                        |ui| {
                            for profile in [Profile::Gaming, Profile::Office, Profile::Firmware] {
                                if profile_buttons::button(ui, profile, self.selected == profile)
                                    .clicked()
                                {
                                    self.select(profile);
                                }
                            }
                        },
                    );
                });
            });
            let line = if let Some(e) = &self.error {
                e.clone()
            } else if self.status.sample_age_ms > 5000 {
                format!(
                    "Hardware sample {:.1}s old · {}",
                    self.status.sample_age_ms as f32 / 1000.,
                    self.status.message
                )
            } else {
                self.status.message.clone()
            };
            ui.add(egui::Label::new(line).truncate());
            ui.add_space(8.);
        } else {
            ui.weak(if self.page == Page::Camera {
                "Camera controls, logo scenes and virtual output"
            } else {
                "Appearance, monitoring and optional sidebar cards"
            });
        }
    }
    fn footer(&mut self, ui: &mut egui::Ui) {
        let dirty = self.dirty();
        let validation = if self.page == Page::Settings {
            self.settings_draft.validate()
        } else {
            self.selected
                .index()
                .map(|i| self.drafts[i].validate())
                .unwrap_or(Ok(()))
        };
        let subject = if self.page == Page::Settings {
            "Settings"
        } else {
            self.selected.label()
        };
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(if dirty {
                    format!("Unsaved changes to {subject}")
                } else {
                    format!("{subject} · saved")
                });
                ui.weak(
                    "Selected profile restores automatically · 0% fan-off · Configurable smoothing",
                );
                if let Err(e) = &validation {
                    ui.label(e);
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let fill = if dirty {
                    ui.visuals().selection.bg_fill
                } else {
                    Color32::from_gray(38)
                };
                // Stable colors and geometry. Routine polling never changes this button.
                let button = egui::Button::new(
                    RichText::new(if self.page == Page::Settings {
                        "Save settings"
                    } else {
                        "Save profile"
                    })
                    .color(if dirty {
                        Color32::WHITE
                    } else {
                        Color32::from_gray(175)
                    }),
                )
                .fill(fill)
                .min_size(Vec2::new(150., 40.));
                let response = ui.add_enabled(
                    dirty
                        && validation.is_ok()
                        && (self.connected || self.page == Page::Settings)
                        && self.command_pending == 0,
                    button,
                );
                self.save_rect = response.rect;
                if response.clicked() {
                    self.save_profile();
                }
            });
        });
        if self.page == Page::Settings {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("Made by R. A. Giampaolo").size(12.).weak());
            });
        }
    }
    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.add_space(8.);
        ui.horizontal(|ui| {
            for (i, label) in ["Appearance", "Telemetry", "System monitors"]
                .iter()
                .enumerate()
            {
                ui.selectable_value(&mut self.settings_section, i, *label);
            }
        });
        ui.add_space(16.);
        egui::Frame::group(ui.style()).inner_margin(20.).show(ui,|ui|{
            ui.set_min_width((ui.available_width()-1.).max(200.));
            match self.settings_section {
                0=>{
                    let old=self.preferences.appearance.clone();
                    ui.label(RichText::new("Theme").strong());
                    ui.horizontal(|ui|{for t in [theme::Theme::Kde,theme::Theme::Black,theme::Theme::White]{ui.selectable_value(&mut self.preferences.appearance.theme,t,t.label());}});
                    ui.add_space(16.);ui.label(RichText::new("Accent color").strong());
                    let mut follow=self.preferences.appearance.accent.is_none();if widgets::checkbox(ui,&mut follow,"Follow KDE accent").changed(){self.preferences.appearance.accent=if follow{None}else{Some(ui.visuals().selection.bg_fill.to_srgba_unmultiplied()[..3].try_into().unwrap())};}
                    if let Some(color)=&mut self.preferences.appearance.accent{ui.color_edit_button_srgb(color);}
                    ui.add_space(16.);ui.label(RichText::new("Background opacity").strong());
                    widgets::slider(ui,"Opacity",&mut self.preferences.appearance.opacity,30..=100,1.," %");
                    ui.weak("Lower values are more transparent. KDE blur remains enabled; text and controls stay opaque.");
                    if old!=self.preferences.appearance{theme::apply_options(ui.ctx(),&self.preferences.appearance,&self.kde_source);}
                }
                1=>{
                    ui.label(RichText::new("dGPU polling interval").strong());
                    widgets::slider(ui,"Interval",&mut self.settings_draft.gpu_poll_seconds,1..=300,1.," s");
                    ui.horizontal(|ui|{for seconds in [1,2,5,10,30,60]{ui.selectable_value(&mut self.settings_draft.gpu_poll_seconds,seconds,format!("{seconds}s"));}});
                    ui.label("Shared by both fans and the optional Gaming usage card. Saved across logins.");
                    ui.weak("Queries run only for an active nvidia-smi curve or an enabled Gaming usage card, while the GPU is D0/active. Sleeping GPUs are skipped.");
                    ui.add_space(16.);ui.label(RichText::new("Power efficiency").strong());
                    ui.label("CPU and power cards update every 3 seconds when enabled. Disabled cards do no periodic work.");
                    ui.label("Camera preview follows the selected capture frame rate while visible. Capture is always opt-in.");
                    ui.weak("Frequent NVIDIA queries can keep an awake GPU active. No nvidia-settings calls are used.");
                }
                _=>{
                    ui.label(RichText::new("System monitor sampling").strong());
                    ui.label("System information is always available on the Monitors page.");
                    widgets::checkbox(ui,&mut self.preferences.cards.power,"Sample battery power and power mode");
                    widgets::checkbox(ui,&mut self.preferences.cards.usage,"Processor usage · CPU, plus dGPU in Gaming");
                    ui.add_space(12.);
                    ui.add_enabled_ui(self.preferences.cards.usage,|ui|{widgets::checkbox(ui,&mut self.settings_draft.gpu_usage_in_gaming,"Enable nvidia-smi for dGPU usage in Gaming");});
                    ui.weak("dGPU usage is opt-in and shares the polling interval. It is never polled outside Gaming or while the GPU is asleep.");
                    ui.weak("Battery watts show battery power flow, not total laptop or wall consumption.");
                }
            }
        });
    }
    fn controls(&mut self, ui: &mut egui::Ui) {
        let selected_index = self.selected.index().unwrap_or(0);
        let enabled = self.selected != Profile::Firmware;
        ui.columns(2, |columns| {
            for (i, column) in columns.iter_mut().enumerate() {
                column.push_id(i, |ui| {
                    egui::Frame::group(ui.style()).inner_margin(12.).show(ui, |ui| {
                        ui.heading(if i == 0 { "CPU fan" } else { "dGPU fan" });
                        let temperatures = &self.status.temperatures;
                        let physical = temperatures.physical_temperature(i);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(physical.map(|t| format!("{t:.0} °C")).unwrap_or_else(||
                                if i == 1 && temperatures.gpu_state.contains("D3") { "Sleeping".into() } else { "Unavailable".into() })).size(28.));
                            ui.separator();
                            ui.label(if self.connected {
                                self.status.requested.map(|v| format!("{} % requested", v[i])).unwrap_or("Firmware".into())
                            } else { "Unknown output".into() });
                        });
                        ui.weak(if i == 0 { format!("CPU sensor · raw {}", raw(self.status.raw_fans[i])) }
                            else { format!("{} · raw {}", temperatures.gpu_state, raw(self.status.raw_fans[i])) });
                        ui.separator();
                        ui.add_enabled_ui(enabled, |ui| {
                            let fan = &mut self.drafts[selected_index].fans[i];
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut fan.mode, Mode::Curve, "Temperature curve");
                                ui.selectable_value(&mut fan.mode, Mode::Manual, "Manual");
                            });
                            ui.add_space(8.);
                            if fan.mode == Mode::Manual {
                                ui.label("Fixed output for this fan");
                                widgets::slider(ui,"Fan output",&mut fan.manual,0..=100,1.," %");
                            } else {
                                ui.horizontal(|ui| {
                                    ui.label("Source");
                                    egui::ComboBox::from_id_salt("sensor").selected_text(fan.sensor.label())
                                        .width(245.).show_ui(ui, |ui| {
                                            for sensor in Sensor::ALL {
                                                ui.selectable_value(&mut fan.sensor, sensor, sensor.label());
                                            }
                                        });
                                });
                                let raw = fan.sensor.temperature(temperatures);
                                let applied = &self.status.config.fans[i];
                                let current = if fan.average_seconds > 0 && fan.sensor == applied.sensor && fan.average_seconds == applied.average_seconds { self.status.control_temperatures[i].or(raw) } else { raw };
                                ui.weak(format!("Selected source: {}", raw.map(|t| format!("{t:.0} °C")).unwrap_or("Unavailable".into())));
                                if fan.average_seconds > 0 { ui.weak(format!("Averaged curve input: {}", current.map(|t| format!("{t:.0} °C")).unwrap_or("Unavailable".into()))); }
                                chart::editor(ui, fan, &mut self.selected_point[i], current);
                                if fan.sensor != Sensor::Cpu {
                                    ui.add_space(8.);
                                    ui.label("When the selected temperature is unavailable");
                                    widgets::slider(ui,"Fallback output",&mut fan.unavailable,0..=100,1.," %");
                                    ui.weak("Set 0% to stop this fan when off, sleeping or unreadable.");
                                }
                                ui.add_space(12.);ui.separator();
                                widgets::slider(ui,"Temperature averaging",&mut fan.average_seconds,0..=60,1.," s");
                                ui.weak("0 = instant readings. Try 5 s to soften brief spikes. Emergency cooling uses raw temperatures.");
                                ui.label(RichText::new("Fan transition").strong());
                                let mut seconds=fan.smoothing.duration_ms as f32/1000.;
                                if widgets::slider(ui,"Duration",&mut seconds,0. ..=10.,0.1," s").changed(){fan.smoothing.duration_ms=(seconds*1000.).round() as u64;}
                                widgets::slider(ui,"Steps",&mut fan.smoothing.steps,1..=20,1.,"");
                                ui.weak("0 seconds = instant. Thermal protection always acts immediately.");
                            }
                        });
                    });
                });
            }
        });
    }
    fn draw(&mut self, ui: &mut egui::Ui) {
        let fill = ui.visuals().panel_fill;
        egui::Panel::left("navigation")
            .exact_size(if self.preferences.sidebar_collapsed {
                68.
            } else {
                210.
            })
            .frame(egui::Frame::NONE.fill(fill).inner_margin(12.))
            .show(ui, |ui| self.sidebar(ui));
        if self.page == Page::Fans {
            egui::Panel::top("header")
                .exact_size(174.)
                .frame(egui::Frame::NONE.fill(fill).inner_margin(12.))
                .show(ui, |ui| self.header(ui));
        }
        if matches!(self.page, Page::Fans | Page::Settings) {
            egui::Panel::bottom("footer")
                .exact_size(if self.page == Page::Settings {
                    104.
                } else {
                    84.
                })
                .frame(egui::Frame::NONE.fill(fill).inner_margin(12.))
                .show(ui, |ui| self.footer(ui));
        }
        egui::CentralPanel::default_margins()
            .frame(egui::Frame::NONE.fill(fill).inner_margin(10.))
            .show(ui, |ui| {
                if self.page == Page::Camera {
                    self.camera.ui(ui, self.demo);
                } else {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| match self.page {
                            Page::Settings => self.settings(ui),
                            Page::Monitors => self.monitors(ui),
                            _ => self.controls(ui),
                        });
                }
            });
    }
}
impl eframe::App for App {
    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        self.camera.close();
        self.blur.take();
    }
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.; 4]
    }
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if let Some(blur) = &mut self.blur {
            blur.dispatch();
        }
        self.capture(ctx);
        self.receive();
        self.camera.poll(ctx);
        while let Some(action) = self.camera.take_action() {
            match action["action"].as_str() {
                Some("show_gui") => {
                    if action["page"].as_str() == Some("camera") {
                        self.page = Page::Camera;
                    }
                    if let Some(blur) = &mut self.blur {
                        blur.activate(action["token"].as_str().unwrap_or_default());
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Some("quit_gui") => {
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Some("request_quit") => self.request_close(ctx, true),
                _ => {}
            }
        }
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
            && !self.demo
            && self.screenshot.is_none()
        {
            self.request_close(ctx, false);
        }
        self.update_close_after_save(ctx);
        if self.close_dialog && !self.allow_close && self.screenshot.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        self.camera.set_visible(ctx, self.page == Page::Camera);
        if self.page == Page::Monitors {
            self.info.update(&self.preferences.cards);
        }
        ctx.request_repaint_after(Duration::from_secs(2));
        if !self.demo && self.pending == 0 && self.last.elapsed() >= Duration::from_secs(2) {
            self.pending += 1;
            let _ = self.tx.send(Work {
                request: Request::Status,
                command: false,
            });
            self.last = Instant::now();
        }
        if self.preferences.appearance.theme == theme::Theme::Kde
            && self.theme_checked.elapsed() >= Duration::from_secs(5)
        {
            let source = theme::kde_source();
            if source != self.kde_source {
                self.kde_source = source;
                theme::apply_options(ctx, &self.preferences.appearance, &self.kde_source);
            }
            self.theme_checked = Instant::now();
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.draw(ui);
        self.camera_close_dialog(ui.ctx());
    }
}
fn raw(v: Option<i32>) -> String {
    v.map(|n| n.to_string()).unwrap_or("unavailable".into())
}
fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args().collect();
    let arg = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
    let socket = arg("--socket").unwrap_or(SOCKET.into());
    let theme = match arg("--theme").as_deref() {
        Some("black") => theme::Theme::Black,
        Some("white") => theme::Theme::White,
        Some("kde") => theme::Theme::Kde,
        _ => preferences::load().appearance.theme,
    };
    let screenshot = arg("--screenshot");
    let demo = args.iter().any(|a| a == "--demo");
    let demo_close_prompt = demo && args.iter().any(|a| a == "--demo-close-prompt");
    let _gui_lock = if !demo && screenshot.is_none() {
        if args.iter().any(|a| a == "--background") {
            let _ = background::connect();
            return Ok(());
        }
        if background::show_existing() {
            return Ok(());
        }
        match background::gui_lock() {
            Some(lock) => Some(lock),
            None => {
                let _ = background::show_existing();
                return Ok(());
            }
        }
    } else {
        None
    };
    let dirty = args.iter().any(|a| a == "--demo-dirty");
    let settings_page = args.iter().any(|a| a == "--settings");
    let camera_page = args.iter().any(|a| a == "--camera");
    let demo_cards = args.iter().any(|a| a == "--demo-cards");
    let demo_stream = args.iter().any(|a| a == "--demo-stream");
    let monitors_page = args.iter().any(|a| a == "--monitors");
    let collapsed = args.iter().any(|a| a == "--collapsed");
    let settings_section = arg("--settings-section").and_then(|s| s.parse::<usize>().ok());
    eframe::run_native(
        "Laptop Control",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_app_id("laptop-control")
                .with_transparent(true)
                .with_icon(
                    eframe::icon_data::from_png_bytes(include_bytes!(
                        "../assets/laptop-control.png"
                    ))
                    .expect("Application icon"),
                )
                .with_inner_size([1380., 980.])
                .with_min_inner_size([1180., 700.]),
            ..Default::default()
        },
        Box::new(move |cc| {
            let mut blur = cc
                .winit_window()
                .and_then(|window| blur::Blur::install(window.clone()));
            if let Some(blur) = &mut blur {
                blur.activate(&std::env::var("XDG_ACTIVATION_TOKEN").unwrap_or_default());
            }
            let mut app = App::new(&cc.egui_ctx, socket, demo, screenshot, theme);
            app.blur = blur;
            if demo_close_prompt {
                app.camera.config.scene_mut().name = "Unsaved scene".into();
                app.close_dialog = true;
            }
            if demo_stream {
                app.require_camera_preview = true;
                app.page = Page::Camera;
                app.camera.demonstration(&cc.egui_ctx);
            }
            if monitors_page {
                app.page = Page::Monitors;
            }
            if collapsed {
                app.preferences.sidebar_collapsed = true;
            }
            if settings_page {
                app.page = Page::Settings;
            }
            if camera_page {
                app.page = Page::Camera;
            }
            if demo_cards {
                app.preferences.cards = preferences::Cards {
                    system: true,
                    power: true,
                    usage: true,
                };
            }
            if let Some(section) = settings_section {
                app.settings_section = section;
            }
            if demo && dirty {
                app.drafts[0].fans[0].curve[1][1] = 35.;
            }
            Ok(Box::new(app))
        }),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn camera_close_can_cancel_and_failed_save_cannot_close() {
        let ctx = egui::Context::default();
        let mut app = App::new(
            &ctx,
            "/tmp/unused.sock".into(),
            true,
            None,
            theme::Theme::Black,
        );
        let saved = app.camera.config.clone();
        app.camera.config.scene_mut().name = "Unsaved scene".into();
        app.request_close(&ctx, true);
        assert!(app.close_dialog && app.quit_background);
        assert!(!app.allow_close);
        app.cancel_close();
        assert!(!app.close_dialog && !app.quit_background && !app.allow_close);
        app.request_close(&ctx, false);
        app.close_after_save = true;
        app.camera.error = Some("Disk full".into());
        app.update_close_after_save(&ctx);
        assert!(app.close_dialog && !app.allow_close);
        app.camera.config = saved;
        app.camera.error = None;
        app.close_after_save = true;
        app.update_close_after_save(&ctx);
        assert!(app.allow_close && !app.close_dialog);
    }
    #[test]
    fn footer_stays_fixed_when_temperature_and_poll_state_change() {
        let ctx = egui::Context::default();
        let mut app = App::new(
            &ctx,
            "/tmp/unused.sock".into(),
            true,
            None,
            theme::Theme::Black,
        );
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1180., 900.),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |ui| app.draw(ui));
        let first = app.save_rect;
        app.status.temperatures.cpu = Some(84.);
        app.status.temperatures.gpu = Some(75.);
        app.status.temperatures.gpu_state = "D0".into();
        app.pending = 1;
        let _ = ctx.run_ui(input(), |ui| app.draw(ui));
        assert_eq!(app.save_rect, first);
        assert!(!app.dirty());
        app.drafts[0].fans[0].manual = 20;
        assert!(app.dirty());
        let _ = ctx.run_ui(input(), |ui| app.draw(ui));
        assert_eq!(app.save_rect, first);
    }
    #[test]
    fn switching_keeps_unsaved_profile_edits() {
        let ctx = egui::Context::default();
        let mut app = App::new(
            &ctx,
            "/tmp/unused.sock".into(),
            true,
            None,
            theme::Theme::Black,
        );
        app.drafts[0].fans[0].manual = 20;
        assert!(app.dirty());
        app.select(Profile::Gaming);
        assert!(!app.dirty());
        app.select(Profile::Office);
        assert!(app.dirty());
        app.save_profile();
        assert!(!app.dirty());
        assert_eq!(app.saved.office.fans[0].manual, 20);
    }
    #[test]
    fn settings_and_sensor_edits_save_independently() {
        let ctx = egui::Context::default();
        let mut app = App::new(
            &ctx,
            "/tmp/unused.sock".into(),
            true,
            None,
            theme::Theme::Black,
        );
        app.drafts[0].fans[0].sensor = Sensor::NvidiaSmi;
        app.drafts[0].fans[1].sensor = Sensor::Cpu;
        assert!(app.dirty());
        app.page = Page::Settings;
        assert!(!app.dirty());
        app.settings_draft.gpu_poll_seconds = 17;
        assert!(app.dirty());
        app.save_profile();
        assert!(!app.dirty());
        assert_eq!(app.saved.settings.gpu_poll_seconds, 17);
        assert!(!app.saved.office.uses_nvidia_smi());
        app.page = Page::Fans;
        assert!(app.dirty());
        app.save_profile();
        assert!(!app.dirty());
        assert!(app.saved.office.uses_nvidia_smi());
        assert_eq!(app.saved.office.fans[1].sensor, Sensor::Cpu);
        assert_eq!(app.saved.settings.gpu_poll_seconds, 17);
    }
}
