use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
};
fn id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LayerKind {
    #[default]
    Image,
    Text,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layer {
    pub id: String,
    pub name: String,
    pub kind: LayerKind,
    pub path: String,
    pub text: String,
    pub font: String,
    pub font_size: f32,
    pub bold: bool,
    pub color: [u8; 4],
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub opacity: f32,
    pub enabled: bool,
}
impl Default for Layer {
    fn default() -> Self {
        Self {
            id: id(),
            name: "Logo".into(),
            kind: LayerKind::Image,
            path: String::new(),
            text: "Your text".into(),
            font: "Sans".into(),
            font_size: 36.,
            bold: false,
            color: [255; 4],
            x: 0.05,
            y: 0.05,
            width: 0.25,
            opacity: 1.,
            enabled: true,
        }
    }
}
impl Layer {
    pub fn text() -> Self {
        Self {
            name: "Text".into(),
            kind: LayerKind::Text,
            width: 0.5,
            ..Self::default()
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scene {
    pub name: String,
    pub layers: Vec<Layer>,
}
impl Default for Scene {
    fn default() -> Self {
        Self {
            name: "Scene 1".into(),
            layers: Vec::new(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraPreset {
    pub name: String,
    pub input: String,
    pub controls: BTreeMap<String, String>,
}
impl Default for CameraPreset {
    fn default() -> Self {
        Self {
            name: "New preset".into(),
            input: String::new(),
            controls: BTreeMap::new(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraConfig {
    pub input: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: String,
    pub mirror: bool,
    #[serde(default)]
    pub scenes: Vec<Scene>,
    pub selected_scene: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<Layer>,
    pub controls: BTreeMap<String, String>,
    pub presets: Vec<CameraPreset>,
    pub selected_preset: Option<usize>,
}
impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            input: String::new(),
            width: 1280,
            height: 720,
            fps: 30,
            format: "MJPG".into(),
            mirror: false,
            scenes: vec![Scene::default()],
            selected_scene: 0,
            overlays: Vec::new(),
            controls: BTreeMap::new(),
            presets: Vec::new(),
            selected_preset: None,
        }
    }
}
impl CameraConfig {
    pub fn migrate(&mut self) {
        if self.scenes.is_empty() {
            self.scenes.push(Scene {
                name: "Scene 1".into(),
                layers: std::mem::take(&mut self.overlays),
            });
        }
        self.selected_scene = self.selected_scene.min(self.scenes.len() - 1);
        self.selected_preset = self.selected_preset.filter(|i| *i < self.presets.len());
        for scene in &mut self.scenes {
            for (i, layer) in scene.layers.iter_mut().enumerate() {
                if layer.name == "Logo" && layer.kind == LayerKind::Image {
                    layer.name = std::path::Path::new(&layer.path)
                        .file_stem()
                        .map(|p| p.to_string_lossy().into_owned())
                        .filter(|p| !p.is_empty())
                        .unwrap_or_else(|| format!("Logo {}", i + 1));
                }
            }
        }
    }
    pub fn scene(&self) -> &Scene {
        &self.scenes[self.selected_scene]
    }
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scenes[self.selected_scene]
    }
    pub fn payload(&self) -> serde_json::Value {
        serde_json::json!({"input":self.input,"width":self.width,"height":self.height,"fps":self.fps,"format":self.format,"mirror":self.mirror,"controls":self.controls,"overlays":self.scene().layers})
    }
    pub fn duplicate(&mut self) {
        let mut scene = self.scene().clone();
        scene.name = format!("{} copy", scene.name);
        for layer in &mut scene.layers {
            layer.id = id();
        }
        self.scenes.push(scene);
        self.selected_scene = self.scenes.len() - 1;
    }
    pub fn store_preset(&mut self) {
        if let Some(i) = self.selected_preset {
            if let Some(preset) = self.presets.get_mut(i) {
                preset.name = preset.name.trim().to_owned();
                if preset.name.is_empty() {
                    preset.name = format!("Preset {}", i + 1);
                }
                preset.input = self.input.clone();
                preset.controls = self.controls.clone();
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn named_presets_save_current_controls_and_migrate_old_files() {
        let mut c: CameraConfig = serde_json::from_str(
            r#"{"input":"/dev/example","scenes":[{"name":"Meeting","layers":[]}]}"#,
        )
        .unwrap();
        c.migrate();
        assert!(c.presets.is_empty());
        c.presets.push(CameraPreset {
            name: " Backlight ".into(),
            ..CameraPreset::default()
        });
        c.selected_preset = Some(0);
        c.controls.insert("brightness".into(), "150".into());
        c.store_preset();
        assert_eq!(c.presets[0].name, "Backlight");
        assert_eq!(c.presets[0].input, "/dev/example");
        assert_eq!(c.presets[0].controls["brightness"], "150");
        c.presets[0].name = "Home".into();
        let mut restored: CameraConfig =
            serde_json::from_slice(&serde_json::to_vec(&c).unwrap()).unwrap();
        assert_eq!(restored, c);
        restored.selected_preset = Some(100);
        restored.migrate();
        assert_eq!(restored.selected_preset, None);
    }
    #[test]
    fn migration_preserves_old_logos_and_controls() {
        let mut c:CameraConfig=serde_json::from_str(r#"{"input":"/dev/video1","overlays":[{"path":"/tmp/a.png","x":1,"y":0,"width":0.2,"opacity":1,"enabled":true}],"controls":{"brightness":"128"}}"#).unwrap();
        c.migrate();
        assert_eq!(c.scene().layers[0].path, "/tmp/a.png");
        assert_eq!(c.controls["brightness"], "128");
        assert!(c.overlays.is_empty());
    }
    #[test]
    fn scenes_and_layers_have_no_fixed_limit() {
        let mut c = CameraConfig::default();
        for _ in 0..130 {
            c.scene_mut().layers.push(Layer::text());
        }
        for _ in 0..100 {
            c.duplicate();
        }
        assert_eq!(c.scenes.len(), 101);
        assert_eq!(c.scene().layers.len(), 130);
        let d: CameraConfig = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(c, d);
        assert_ne!(c.scenes[0].layers[0].id, c.scenes[1].layers[0].id);
    }
}
