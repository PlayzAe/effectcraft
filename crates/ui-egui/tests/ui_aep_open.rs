//! End-to-end integration test for opening and rendering an After Effects project (.aep).

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;

#[test]
fn opens_and_displays_after_effects_project_in_ui() {
    let path = r"C:\Users\Moses\Downloads\MGPack v2\Shock pack\01_Instagram Stories\Instagram_stories_1.aep";
    if !std::path::Path::new(path).exists() {
        return;
    }

    let mut s = Session::default();
    let res = s.execute("file.open", json!({"path": path})).expect("file.open succeeds on AEP");
    assert_eq!(res["type"], "aep");
    assert!(res["comps"].as_u64().unwrap_or(0) >= 3);

    let active_id = s.active_comp_id().expect("has active comp");
    let comp = s.project.comp(active_id).expect("comp exists");
    assert_eq!(comp.width, 1080);
    assert_eq!(comp.height, 1920);
    assert_eq!(comp.layers.len(), 24);

    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(4);

    // Verify Project panel has loaded items
    assert!(h.state().session.project.items.len() >= 3);
    assert!(h.state().session.path.is_some());
}
