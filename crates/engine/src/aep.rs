//! Import and decompile Adobe After Effects project files (`.aep` binary RIFX format and
//! `.aepx` XML format).
//!
//! An `.aep` file is a RIFX (big-endian RIFF) or RIFF container with form type `Egg!`. Inside
//! is a chunk hierarchy:
//! - `Fold`: Root project folder
//! - `Item`: Project items (folders, compositions, footage, solids)
//! - `Sfdr`: Subfolder containing child `Item`s
//! - `cdta`: Composition metadata (width, height, frame rate, duration in frames)
//! - `Layr` / `SLay` / `CLay`: Layers (AVLayer, Solid Layer, Camera Layer)
//! - `ldta`: Layer metadata (source item id, layer index, timings, layer name)
//! - `tdgp`: Property stream groups (transform, effects, text, shape contents)
//!
//! An `.aepx` file is the XML serialization counterpart. Both are decoded into standard
//! [`effectcraft_project::Project`] documents.

use std::collections::HashMap;
use std::sync::Arc;

use effectcraft_color::Label;
use effectcraft_project::build;
use effectcraft_project::{Comp, Footage, FootageKind, Item, ItemId, ItemKind, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

/// Detailed report on assets, fonts, and plugins found during `.aep` import.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct AepReport {
    /// Project file path if known.
    pub project_path: Option<String>,
    /// Font families referenced in the project.
    pub fonts: Vec<String>,
    /// Fonts missing from the local system.
    pub missing_fonts: Vec<String>,
    /// Paths or names of missing footage files.
    pub missing_footage: Vec<String>,
    /// Total footage items discovered.
    pub total_footage_count: usize,
    /// Number of missing footage items.
    pub missing_footage_count: usize,
    /// Total compositions discovered.
    pub comp_count: usize,
    /// All effects/plugins used in the project (friendly display names).
    pub plugins: Vec<String>,
    /// Custom / third-party plugins detected (e.g. Cycore CC, Trapcode, Sapphire, Video Copilot, etc.).
    pub custom_plugins: Vec<String>,
}

impl AepReport {
    /// Whether the report has any missing assets, missing fonts, or custom plugins to alert the user about.
    pub fn has_issues(&self) -> bool {
        !self.custom_plugins.is_empty() || self.missing_footage_count > 0 || !self.missing_fonts.is_empty()
    }
}

/// Whether `bytes` has the binary After Effects project signature (RIFX/RIFF `Egg!`).
pub fn is_aep(bytes: &[u8]) -> bool {
    if bytes.len() < 12 {
        return false;
    }
    let magic = &bytes[0..4];
    let form = &bytes[8..12];
    (magic == b"RIFX" || magic == b"RIFF") && form == b"Egg!"
}

/// Whether `bytes` looks like an After Effects XML project (`.aepx`).
pub fn is_aepx(bytes: &[u8]) -> bool {
    let s = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let t = s.trim_start_matches('\u{feff}').trim_start();
    (t.starts_with("<?xml") || t.starts_with("<AfterEffects") || t.starts_with("<Egg"))
        && (t.contains("AfterEffects") || t.contains("Egg") || t.contains("bdata") || t.contains("string id=\"Utf8\""))
}

/// A node in a parsed RIFF / RIFX chunk tree.
#[derive(Clone, Debug)]
pub enum ChunkNode<'a> {
    Chunk {
        id: [u8; 4],
        data: &'a [u8],
    },
    List {
        id: [u8; 4],
        list_type: [u8; 4],
        children: Vec<ChunkNode<'a>>,
    },
}

/// Parse a RIFX or RIFF binary stream into a tree of chunks.
pub fn parse_rifx(bytes: &[u8]) -> Result<Vec<ChunkNode<'_>>, String> {
    if bytes.len() < 12 {
        return Err("AEP file too small".into());
    }
    let magic = &bytes[0..4];
    if magic != b"RIFX" && magic != b"RIFF" {
        return Err(format!("not a RIFF/RIFX file: {:?}", magic));
    }
    let be = magic == b"RIFX";
    let form = &bytes[8..12];
    if form != b"Egg!" {
        return Err(format!("not an After Effects Egg! container: {:?}", form));
    }
    parse_chunks(&bytes[12..], be, 0)
}

fn parse_chunks<'a>(data: &'a [u8], be: bool, depth: usize) -> Result<Vec<ChunkNode<'a>>, String> {
    if depth > 64 {
        return Ok(Vec::new());
    }
    let mut nodes = Vec::new();
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let mut id = [0u8; 4];
        id.copy_from_slice(&data[pos..pos + 4]);
        let len = if be {
            u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]) as usize
        } else {
            u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]) as usize
        };
        let cstart = pos + 8;
        let cend = (cstart + len).min(data.len());
        let pad = len % 2;
        pos = (cend + pad).min(data.len() + 1);

        if &id == b"LIST" {
            if len >= 4 && cstart + 4 <= cend {
                let mut ltype = [0u8; 4];
                ltype.copy_from_slice(&data[cstart..cstart + 4]);
                let sub = parse_chunks(&data[cstart + 4..cend], be, depth + 1).unwrap_or_default();
                nodes.push(ChunkNode::List { id, list_type: ltype, children: sub });
            }
        } else {
            nodes.push(ChunkNode::Chunk { id, data: &data[cstart..cend] });
        }
        if pos == 0 || pos >= data.len() {
            break;
        }
    }
    Ok(nodes)
}

/// Extracted composition information before turning into project items.
#[derive(Clone, Debug)]
struct DecodedComp {
    name: String,
    width: u32,
    height: u32,
    pixel_aspect: f64,
    frame_rate: f64,
    duration_frames: u32,
    background: [f32; 3],
    layers: Vec<DecodedLayer>,
    raw_item_id: u32,
    parent_folder: Option<String>,
}

/// Extracted footage / media information.
#[derive(Clone, Debug)]
struct DecodedFootage {
    id: u32,
    name: String,
    path: String,
    width: u32,
    height: u32,
    frame_rate: f64,
    _duration_frames: u32,
    parent_folder: Option<String>,
    is_solid: bool,
}

/// Extracted layer information.
#[derive(Clone, Debug)]
struct DecodedLayer {
    name: String,
    layer_type: [u8; 4],
    in_sec: f64,
    out_sec: f64,
    start_sec: f64,
    source_item_id: u32,
    three_d: bool,
    is_text: bool,
    is_shape: bool,
    text_font: Option<String>,
    _effects: Vec<String>,
}

/// Scan bytes for `/CoolTypeFont` declarations and extract font family names.
pub fn extract_aep_fonts(bytes: &[u8]) -> Vec<String> {
    let mut fonts = Vec::new();
    let marker = b"/CoolTypeFont";
    let mut pos = 0;
    while pos + marker.len() <= bytes.len() {
        if let Some(idx) = bytes[pos..].windows(marker.len()).position(|w| w == marker) {
            let start = pos + idx + marker.len();
            pos = start;
            let search_len = 128.min(bytes.len().saturating_sub(start));
            let window = &bytes[start..start + search_len];
            if let Some(bom_idx) = window.windows(2).position(|w| w == b"\xfe\xff") {
                let text_start = start + bom_idx + 2;
                let max_end = (text_start + 256).min(bytes.len());
                if let Some(close_idx) = bytes[text_start..max_end].iter().position(|&b| b == b')') {
                    let u16_bytes = &bytes[text_start..text_start + close_idx];
                    let u16s: Vec<u16> = u16_bytes.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                    let font_name = String::from_utf16_lossy(&u16s).trim().to_string();
                    if !font_name.is_empty() && font_name != "AdobeInvisFont" && !fonts.contains(&font_name) {
                        fonts.push(font_name);
                    }
                }
            }
        } else {
            break;
        }
    }
    fonts
}

/// Extract all effects and plugins referenced in the project from `Pefl` (pjef) and `EfdG` (EfDf).
pub fn extract_aep_plugins(bytes: &[u8]) -> (Vec<String>, Vec<String>) {
    let mut all_plugins = Vec::new();
    let mut custom_plugins = Vec::new();

    let match_map: HashMap<&str, &str> = [
        ("ADBE 4ColorGradient", "4-Color Gradient"),
        ("ADBE Box Blur2", "Fast Box Blur"),
        ("ADBE Checkbox Control", "Checkbox Control"),
        ("ADBE Color Control", "Color Control"),
        ("ADBE CurvesCustom", "Curves"),
        ("ADBE Drop Shadow", "Drop Shadow"),
        ("ADBE Easy Levels2", "Levels"),
        ("ADBE Fill", "Fill"),
        ("ADBE Gaussian Blur 2", "Gaussian Blur"),
        ("ADBE Geometry2", "Transform"),
        ("ADBE Glo2", "Glow"),
        ("ADBE Grid", "Grid"),
        ("ADBE Linear Wipe", "Linear Wipe"),
        ("ADBE Noise2", "Noise"),
        ("ADBE PhotoFilterPS", "Photo Filter"),
        ("ADBE Sharpen", "Sharpen"),
        ("ADBE Slider Control", "Slider Control"),
        ("ADBE Tile", "Motion Tile"),
        ("ADBE Tint", "Tint"),
        ("ADBE Tritone", "Tritone"),
        ("ADBE Vibrance", "Vibrance"),
        ("CS Composite", "CC Composite"),
    ].into_iter().collect();

    // Dynamically extract friendly display names from EfDf lists in EfdG
    let mut dynamic_names: HashMap<String, String> = HashMap::new();
    let mut pos = 0;
    while pos + 4 <= bytes.len() {
        if let Some(idx) = bytes[pos..].windows(4).position(|w| w == b"EfDf") {
            let efdf_start = pos + idx;
            pos = efdf_start + 4;
            let window_len = 3000.min(bytes.len() - efdf_start);
            let window = &bytes[efdf_start..efdf_start + window_len];

            let mut match_name = String::new();
            if let Some(mn_pos) = window.windows(4).position(|w| w == b"tdmn") {
                if mn_pos + 8 <= window.len() {
                    let len = u32::from_be_bytes([window[mn_pos + 4], window[mn_pos + 5], window[mn_pos + 6], window[mn_pos + 7]]) as usize;
                    let m_end = (mn_pos + 8 + len).min(window.len());
                    match_name = String::from_utf8_lossy(&window[mn_pos + 8..m_end]).trim_matches('\0').trim().to_string();
                }
            }

            let mut display_name = String::new();
            if let Some(fn_pos) = window.windows(4).position(|w| w == b"fnam") {
                if let Some(u_pos) = window[fn_pos..].windows(4).position(|w| w == b"Utf8") {
                    let u_start = fn_pos + u_pos;
                    if u_start + 8 <= window.len() {
                        let len = u32::from_be_bytes([window[u_start + 4], window[u_start + 5], window[u_start + 6], window[u_start + 7]]) as usize;
                        let u_end = (u_start + 8 + len).min(window.len());
                        display_name = String::from_utf8_lossy(&window[u_start + 8..u_end]).trim_matches('\0').trim().to_string();
                    }
                }
            }

            if !match_name.is_empty() && !display_name.is_empty() {
                dynamic_names.insert(match_name, display_name);
            }
        } else {
            break;
        }
    }

    // Scan pjef chunks (Project Effect List)
    let marker = b"pjef";
    pos = 0;
    while pos + 8 <= bytes.len() {
        if let Some(idx) = bytes[pos..].windows(4).position(|w| w == marker) {
            let chunk_pos = pos + idx;
            pos = chunk_pos + 4;
            if chunk_pos + 8 <= bytes.len() {
                let len = u32::from_be_bytes([bytes[chunk_pos + 4], bytes[chunk_pos + 5], bytes[chunk_pos + 6], bytes[chunk_pos + 7]]) as usize;
                let data_start = chunk_pos + 8;
                let data_end = (data_start + len).min(bytes.len());
                let raw_name = String::from_utf8_lossy(&bytes[data_start..data_end]).trim_matches('\0').trim().to_string();
                pos = data_end + (len % 2);

                if !raw_name.is_empty() {
                    let friendly = dynamic_names.get(&raw_name)
                        .map(|s| s.as_str())
                        .or_else(|| match_map.get(raw_name.as_str()).copied())
                        .unwrap_or(raw_name.as_str())
                        .to_string();

                    if !all_plugins.contains(&friendly) {
                        all_plugins.push(friendly.clone());
                    }

                    let is_custom = !raw_name.starts_with("ADBE ")
                        || raw_name.starts_with("CC ")
                        || raw_name.starts_with("CS ")
                        || friendly.starts_with("CC ")
                        || is_third_party_name(&friendly);

                    if is_custom && !custom_plugins.contains(&friendly) {
                        custom_plugins.push(friendly);
                    }
                }
            }
        } else {
            break;
        }
    }

    (all_plugins, custom_plugins)
}

fn is_third_party_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("cc ")
        || lower.starts_with("rg ")
        || lower.starts_with("tc_")
        || lower.starts_with("s_")
        || lower.contains("particular")
        || lower.contains("form")
        || lower.contains("optical flares")
        || lower.contains("element 3d")
        || lower.contains("saber")
        || lower.contains("twitch")
        || lower.contains("plexus")
        || lower.contains("sapphire")
        || lower.contains("magic bullet")
        || lower.contains("boris")
        || lower.contains("mocha")
}

/// Decode an After Effects `.aep` project file into an [`effectcraft_project::Project`].
pub fn from_aep(bytes: &[u8]) -> Result<Project, String> {
    from_aep_with_report(bytes, None).map(|(p, _)| p)
}

/// Decode an After Effects `.aep` project file and produce a detailed report.
pub fn from_aep_with_report(bytes: &[u8], project_path: Option<&std::path::Path>) -> Result<(Project, AepReport), String> {
    let tree = parse_rifx(bytes)?;
    let mut comps = Vec::new();
    let mut folders = Vec::new();
    let mut footage = Vec::new();

    scan_nodes(&tree, None, project_path, &mut comps, &mut folders, &mut footage);

    if comps.is_empty() {
        return Err("no compositions found in After Effects project".into());
    }

    let detected_fonts = extract_aep_fonts(bytes);
    let (all_plugins, custom_plugins) = extract_aep_plugins(bytes);
    build_project(comps, folders, footage, detected_fonts, all_plugins, custom_plugins, project_path)
}

/// Intelligently resolve footage file paths: checks local disk first, then attempts relinking
/// relative to the project directory or common subfolders (`(Footage)`, `Footage`, etc.).
fn resolve_footage_path(file_path: &str, project_path: Option<&std::path::Path>) -> String {
    if file_path.is_empty() {
        return String::new();
    }
    let p = std::path::Path::new(file_path);
    if p.exists() {
        return file_path.to_string();
    }
    let Some(proj_dir) = project_path.and_then(|p| p.parent()) else {
        return file_path.to_string();
    };

    if let Some(file_name) = p.file_name() {
        let candidate = proj_dir.join(file_name);
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }

        for sub in &["(Footage)", "Footage", "footage", "Media", "media", "Assets", "assets", "Source", "source"] {
            let candidate = proj_dir.join(sub).join(file_name);
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }

    let norm = file_path.replace('\\', "/");
    for marker in &["(Footage)/", "Footage/", "footage/", "media/", "assets/"] {
        if let Some(idx) = norm.find(marker) {
            let suffix = &norm[idx..];
            let candidate = proj_dir.join(suffix);
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }

    if let Some(file_name) = p.file_name().and_then(|f| f.to_str()) {
        if let Some(found) = find_file_in_dir(proj_dir, file_name, 0, 3) {
            return found.to_string_lossy().to_string();
        }
    }

    file_path.to_string()
}

fn find_file_in_dir(dir: &std::path::Path, target: &str, depth: usize, max_depth: usize) -> Option<std::path::PathBuf> {
    if depth > max_depth {
        return None;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return None };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.eq_ignore_ascii_case(target) {
                    return Some(path);
                }
            }
        } else if path.is_dir() {
            if let Some(found) = find_file_in_dir(&path, target, depth + 1, max_depth) {
                return Some(found);
            }
        }
    }
    None
}

fn scan_nodes(
    nodes: &[ChunkNode<'_>],
    parent_folder: Option<String>,
    project_path: Option<&std::path::Path>,
    comps: &mut Vec<DecodedComp>,
    folders: &mut Vec<(String, Option<String>)>,
    footage: &mut Vec<DecodedFootage>,
) {
    for node in nodes {
        if let ChunkNode::List { list_type, children, .. } = node {
            if list_type == b"Item" {
                scan_item(children, parent_folder.clone(), project_path, comps, folders, footage);
            } else {
                scan_nodes(children, parent_folder.clone(), project_path, comps, folders, footage);
            }
        }
    }
}

fn extract_alas_path(chunks: &[ChunkNode<'_>]) -> Option<String> {
    for c in chunks {
        match c {
            ChunkNode::Chunk { id, data } => {
                if id == b"alas" {
                    if let Some(p) = parse_alas_json(data) {
                        return Some(p);
                    }
                }
            }
            ChunkNode::List { children, .. } => {
                if let Some(p) = extract_alas_path(children) {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn parse_alas_json(data: &[u8]) -> Option<String> {
    let key = b"\"fullpath\":\"";
    let idx = data.windows(key.len()).position(|w| w == key)?;
    let start = idx + key.len();
    let mut end = start;
    let mut escaped = false;
    while end < data.len() {
        if escaped {
            escaped = false;
        } else if data[end] == b'\\' {
            escaped = true;
        } else if data[end] == b'"' {
            break;
        }
        end += 1;
    }
    let raw = std::str::from_utf8(data.get(start..end)?).ok()?;
    let unescaped = raw.replace(r"\\", r"\").replace(r#"\""#, r#"""#);
    if !unescaped.is_empty() { Some(unescaped) } else { None }
}


fn scan_item(
    chunks: &[ChunkNode<'_>],
    parent_folder: Option<String>,
    project_path: Option<&std::path::Path>,
    comps: &mut Vec<DecodedComp>,
    folders: &mut Vec<(String, Option<String>)>,
    footage: &mut Vec<DecodedFootage>,
) {
    let mut name = String::new();
    let mut cdta: Option<&[u8]> = None;
    let mut idta: Option<&[u8]> = None;
    let mut iide: Option<u32> = None;
    let mut subfolder: Option<&[ChunkNode<'_>]> = None;
    let mut layer_chunks = Vec::new();

    for c in chunks {
        match c {
            ChunkNode::Chunk { id, data } => {
                if id == b"Utf8" {
                    let s = String::from_utf8_lossy(data).trim_matches('\0').trim().to_string();
                    if !s.is_empty() {
                        name = s;
                    }
                } else if id == b"Utf1" && name.is_empty() {
                    let u16s: Vec<u16> = data.chunks_exact(2).map(|b| u16::from_be_bytes([b[0], b[1]])).collect();
                    let s = String::from_utf16_lossy(&u16s).trim_matches('\0').trim().to_string();
                    if !s.is_empty() {
                        name = s;
                    }
                } else if id == b"cdta" {
                    cdta = Some(data);
                } else if id == b"idta" {
                    idta = Some(data);
                } else if id == b"iide" && data.len() >= 4 {
                    iide = Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]));
                }
            }
            ChunkNode::List { list_type, children, .. } => {
                if list_type == b"Sfdr" {
                    subfolder = Some(children);
                } else if list_type == b"Layr"
                    || list_type == b"SLay"
                    || list_type == b"CLay"
                    || list_type == b"SecL"
                    || list_type == b"AVLa"
                    || list_type.ends_with(b"Lay")
                {
                    layer_chunks.push((list_type, children.as_slice()));
                }
            }
        }
    }

    let raw_item_id = if let Some(d) = idta.filter(|d| d.len() >= 20) {
        let sub_id = u32::from_be_bytes([d[16], d[17], d[18], d[19]]);
        if sub_id > 0 { sub_id } else { iide.unwrap_or(0) }
    } else if let Some(lid) = iide {
        lid
    } else if let Some(d) = idta.filter(|d| d.len() >= 4) {
        u32::from_be_bytes([d[0], d[1], d[2], d[3]])
    } else {
        0
    };

    if let Some(sub) = subfolder {
        let folder_name = if name.is_empty() { "Folder".to_string() } else { name };
        folders.push((folder_name.clone(), parent_folder));
        scan_nodes(sub, Some(folder_name), project_path, comps, folders, footage);
    } else if let Some(cd) = cdta {
        let comp_name = if name.is_empty() { format!("Comp {}", comps.len() + 1) } else { name };
        let (w, h, fps, dur_frames, bg) = parse_cdta(cd);

        let mut decoded_layers = Vec::new();
        for (ltype, lchildren) in layer_chunks {
            let layer = parse_layer(*ltype, lchildren);
            decoded_layers.push(layer);
        }

        comps.push(DecodedComp {
            name: comp_name,
            width: w,
            height: h,
            pixel_aspect: 1.0,
            frame_rate: fps,
            duration_frames: dur_frames,
            background: bg,
            layers: decoded_layers,
            raw_item_id,
            parent_folder,
        });
    } else {
        let raw_file_path = extract_alas_path(chunks).unwrap_or_default();
        let file_path = if !raw_file_path.is_empty() {
            resolve_footage_path(&raw_file_path, project_path)
        } else {
            String::new()
        };
        let is_solid = parent_folder.as_deref() == Some("Solids") || file_path.is_empty();
        let fname = if !name.is_empty() {
            name
        } else if !file_path.is_empty() {
            std::path::Path::new(&file_path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "Footage".into())
        } else {
            "Item".into()
        };

        footage.push(DecodedFootage {
            id: raw_item_id,
            name: fname,
            path: file_path,
            width: 1920,
            height: 1080,
            frame_rate: 30.0,
            _duration_frames: 300,
            parent_folder,
            is_solid,
        });
    }
}

/// Extract composition metadata from the `cdta` chunk.
fn parse_cdta(cdta: &[u8]) -> (u32, u32, f64, u32, [f32; 3]) {
    let mut w = 1920;
    let mut h = 1080;
    let mut fps = 30.0;
    let mut duration_frames = 300;
    let mut bg = [0.0f32, 0.0f32, 0.0f32];

    if cdta.len() >= 55 {
        bg = [
            cdta[52] as f32 / 255.0,
            cdta[53] as f32 / 255.0,
            cdta[54] as f32 / 255.0,
        ];
    }

    if cdta.len() >= 144 {
        let cw = u16::from_be_bytes([cdta[140], cdta[141]]) as u32;
        let ch = u16::from_be_bytes([cdta[142], cdta[143]]) as u32;
        if cw > 0 && ch > 0 && cw <= 32768 && ch <= 32768 {
            w = cw;
            h = ch;
        }
    }

    if cdta.len() >= 160 {
        let int_fps = u16::from_be_bytes([cdta[156], cdta[157]]) as f64;
        let frac_fps = u16::from_be_bytes([cdta[158], cdta[159]]) as f64 / 65536.0;
        let raw_fps = int_fps + frac_fps;
        if (1.0..=240.0).contains(&raw_fps) {
            fps = raw_fps;
        }
    } else if cdta.len() >= 172 {
        let raw_fps = u32::from_be_bytes([cdta[168], cdta[169], cdta[170], cdta[171]]);
        if (1..=240).contains(&raw_fps) {
            fps = raw_fps as f64;
        }
    }

    let mut time_base = 30720.0f64;
    if cdta.len() >= 12 {
        let tb = u16::from_be_bytes([cdta[10], cdta[11]]) as f64;
        if tb >= 100.0 && tb <= 100_000.0 {
            time_base = tb;
        }
    }

    if cdta.len() >= 48 {
        let dur_ticks = u32::from_be_bytes([cdta[44], cdta[45], cdta[46], cdta[47]]);
        if dur_ticks > 0 && dur_ticks < 100_000_000 {
            let dur_sec = dur_ticks as f64 / time_base;
            duration_frames = (dur_sec * fps).round().max(1.0) as u32;
        }
    }
    if (duration_frames == 0 || duration_frames > 1_000_000) && cdta.len() >= 20 {
        let df = u32::from_be_bytes([cdta[16], cdta[17], cdta[18], cdta[19]]);
        if df > 0 && df < 1_000_000 {
            duration_frames = df;
        }
    }

    if w == 0 || h == 0 {
        w = 1920;
        h = 1080;
    }

    (w, h, fps, duration_frames.max(1), bg)
}


/// Extract layer metadata from a layer's child chunks.
fn parse_layer(layer_type: [u8; 4], chunks: &[ChunkNode<'_>]) -> DecodedLayer {
    let mut name = String::new();
    let mut ldta: Option<&[u8]> = None;
    let mut is_text = false;
    let mut is_shape = false;
    let mut three_d = layer_type == *b"CLay";
    let mut effects = Vec::new();
    let mut text_font = None;

    for c in chunks {
        match c {
            ChunkNode::Chunk { id, data } => {
                if id == b"Utf8" && !data.is_empty() {
                    let s = String::from_utf8_lossy(data).trim_matches('\0').trim().to_string();
                    if !s.is_empty() {
                        name = s;
                    }
                } else if id == b"ldta" {
                    ldta = Some(data);
                }
            }
            ChunkNode::List { list_type, children, .. } => {
                if list_type == b"tdgp" {
                    inspect_tdgp(children, &mut is_text, &mut is_shape, &mut three_d, &mut effects, &mut text_font);
                }
            }
        }
    }

    let mut time_base = 30720u32;
    let mut in_ticks = 0u32;
    let mut out_ticks = 30720u32 * 10;
    let mut out_time_base = 30720u32;
    let mut source_item_id = 0u32;

    if let Some(ld) = ldta {
        if ld.len() >= 32 {
            let tb = u32::from_be_bytes([ld[16], ld[17], ld[18], ld[19]]);
            if tb > 0 {
                time_base = tb;
            }
            in_ticks = u32::from_be_bytes([ld[20], ld[21], ld[22], ld[23]]);
            let otb = u32::from_be_bytes([ld[24], ld[25], ld[26], ld[27]]);
            out_time_base = if otb > 0 { otb } else { time_base };
            out_ticks = u32::from_be_bytes([ld[28], ld[29], ld[30], ld[31]]);
        }
        if ld.len() >= 44 {
            source_item_id = u32::from_be_bytes([ld[40], ld[41], ld[42], ld[43]]);
        }
        if name.is_empty() && ld.len() >= 96 {
            let sub = &ld[64..96];
            let end = sub.iter().position(|&b| b == 0).unwrap_or(sub.len());
            let cand = String::from_utf8_lossy(&sub[..end]).trim().to_string();
            if !cand.is_empty() {
                name = cand;
            }
        }
    }

    if name.is_empty() {
        name = match &layer_type {
            b"SLay" => "Solid".into(),
            b"CLay" => "Camera".into(),
            b"SecL" => "Markers".into(),
            _ if is_text => "Text".into(),
            _ if is_shape => "Shape Layer".into(),
            _ => "Layer".into(),
        };
    }

    let in_sec = in_ticks as f64 / time_base as f64;
    let mut out_sec = out_ticks as f64 / out_time_base as f64;
    if out_sec <= in_sec {
        out_sec = in_sec + 10.0;
    }

    DecodedLayer {
        name,
        layer_type,
        in_sec,
        out_sec,
        start_sec: in_sec,
        source_item_id,
        three_d,
        is_text,
        is_shape,
        text_font,
        _effects: effects,
    }
}

fn inspect_tdgp(
    chunks: &[ChunkNode<'_>],
    is_text: &mut bool,
    is_shape: &mut bool,
    three_d: &mut bool,
    effects: &mut Vec<String>,
    text_font: &mut Option<String>,
) {
    for c in chunks {
        match c {
            ChunkNode::Chunk { id, data } => {
                if id == b"tdmn" {
                    let mn = String::from_utf8_lossy(data);
                    if mn.contains("ADBE Text Properties") {
                        *is_text = true;
                    } else if mn.contains("Shape Layer") || mn.contains("ADBE Vector") {
                        *is_shape = true;
                    }
                } else if id == b"tdsn" && !data.is_empty() {
                    let sn = String::from_utf8_lossy(data).trim_matches('\0').trim().to_string();
                    if !sn.is_empty() && sn != "Transform" && sn != "Compositing Options" {
                        effects.push(sn);
                    }
                } else if id == b" /98" && text_font.is_none() {
                    let fonts = extract_aep_fonts(data);
                    if let Some(first) = fonts.into_iter().next() {
                        *text_font = Some(first);
                    }
                }
            }
            ChunkNode::List { children, .. } => {
                inspect_tdgp(children, is_text, is_shape, three_d, effects, text_font);
            }
        }
    }
}


/// Build an [`effectcraft_project::Project`] and [`AepReport`] from the decoded components.
fn build_project(
    decoded_comps: Vec<DecodedComp>,
    decoded_folders: Vec<(String, Option<String>)>,
    decoded_footage: Vec<DecodedFootage>,
    fonts: Vec<String>,
    plugins: Vec<String>,
    custom_plugins: Vec<String>,
    project_path: Option<&std::path::Path>,
) -> Result<(Project, AepReport), String> {
    let mut project = Project::default();
    let comp_count = decoded_comps.len();
    let mut folder_ids: HashMap<String, ItemId> = HashMap::new();
    let mut missing_footage = Vec::new();

    // 1. Create folders
    for (fname, parent_name) in decoded_folders {
        let parent = parent_name.as_deref().and_then(|p| folder_ids.get(p)).copied();
        let fid = ItemId(project.alloc());
        project.items.insert(
            fid,
            Item {
                id: fid,
                name: fname.clone(),
                label: Label::Yellow,
                comment: String::new(),
                parent,
                kind: ItemKind::Folder,
                proxy: None,
            },
        );
        folder_ids.insert(fname, fid);
    }

    // Default Solids folder for any solid layers
    let solids_folder = if let Some(&id) = folder_ids.get("Solids") {
        id
    } else {
        let fid = ItemId(project.alloc());
        project.items.insert(
            fid,
            Item {
                id: fid,
                name: "Solids".into(),
                label: Label::Yellow,
                comment: String::new(),
                parent: None,
                kind: ItemKind::Folder,
                proxy: None,
            },
        );
        folder_ids.insert("Solids".into(), fid);
        fid
    };

    // 2. Pre-allocate comp item IDs so precomposing references resolve
    let mut comp_ids: HashMap<String, ItemId> = HashMap::new();
    let mut comp_by_raw_id: HashMap<u32, ItemId> = HashMap::new();
    for c in &decoded_comps {
        let cid = ItemId(project.alloc());
        comp_ids.insert(c.name.clone(), cid);
        if c.raw_item_id > 0 {
            comp_by_raw_id.insert(c.raw_item_id, cid);
        }
    }

    // 3. Create footage items
    let mut footage_ids: HashMap<u32, ItemId> = HashMap::new();
    let mut footage_by_name: HashMap<String, ItemId> = HashMap::new();
    let total_footage = decoded_footage.len();

    for dfoot in decoded_footage {
        let parent = dfoot.parent_folder.as_deref().and_then(|p| folder_ids.get(p)).copied();
        let fid = ItemId(project.alloc());

        if dfoot.is_solid {
            let solid = Solid {
                color: [0.5, 0.5, 0.5],
                width: dfoot.width,
                height: dfoot.height,
                pixel_aspect: 1.0,
            };
            project.items.insert(
                fid,
                Item {
                    id: fid,
                    name: dfoot.name.clone(),
                    label: Label::Red,
                    comment: String::new(),
                    parent: parent.or(Some(solids_folder)),
                    kind: ItemKind::Solid(solid),
                    proxy: None,
                },
            );
        } else {
            let path_exists = !dfoot.path.is_empty() && std::path::Path::new(&dfoot.path).exists();
            let missing = !dfoot.path.is_empty() && !path_exists;
            if missing {
                missing_footage.push(if !dfoot.name.is_empty() { dfoot.name.clone() } else { dfoot.path.clone() });
            }

            let p_lower = dfoot.path.to_ascii_lowercase();
            let kind = if p_lower.ends_with(".wav") || p_lower.ends_with(".mp3") || p_lower.ends_with(".aac") || p_lower.ends_with(".m4a") {
                FootageKind::Audio
            } else if p_lower.ends_with(".mp4") || p_lower.ends_with(".mov") || p_lower.ends_with(".avi") || p_lower.ends_with(".mkv") {
                FootageKind::Video
            } else {
                FootageKind::Still
            };

            let label = match kind {
                FootageKind::Audio => Label::SeaFoam,
                FootageKind::Video => Label::Aqua,
                FootageKind::Still => Label::Lavender,
                _ => Label::Lavender,
            };

            let footage_obj = Footage {
                path: dfoot.path.clone(),
                kind,
                width: dfoot.width,
                height: dfoot.height,
                pixel_aspect: 1.0,
                frame_rate: FrameRate::from_f64(dfoot.frame_rate),
                native_rate: None,
                duration: Tick::from_seconds_f64(10.0),
                has_video: kind != FootageKind::Audio,
                has_audio: kind == FootageKind::Audio || kind == FootageKind::Video,
                alpha: effectcraft_project::AlphaMode::Straight,
                premul_color: [0.0, 0.0, 0.0],
                loop_count: 1,
                codec: String::new(),
                missing,
                sequence: Vec::new(),
                color_profile: None,
                layer: None,
                fields: Default::default(),
                invert_alpha: false,
                linear_light: false,
                data: None,
                page: 0,
            };

            project.items.insert(
                fid,
                Item {
                    id: fid,
                    name: dfoot.name.clone(),
                    label,
                    comment: String::new(),
                    parent,
                    kind: ItemKind::Footage(footage_obj),
                    proxy: None,
                },
            );
        }

        if dfoot.id > 0 {
            footage_ids.insert(dfoot.id, fid);
        }
        footage_by_name.insert(dfoot.name, fid);
    }

    // 4. Build each composition
    for dcomp in decoded_comps {
        let cid = match comp_ids.get(&dcomp.name) {
            Some(&id) => id,
            None => ItemId(project.alloc()),
        };
        let frame_rate = FrameRate::from_f64(dcomp.frame_rate);
        let duration = frame_rate.tick_of(dcomp.duration_frames.max(1) as i64);

        let mut comp = Comp {
            width: dcomp.width,
            height: dcomp.height,
            pixel_aspect: dcomp.pixel_aspect,
            frame_rate,
            duration,
            display_start: Tick::ZERO,
            background: dcomp.background,
            work_area: (Tick::ZERO, duration),
            layers: Vec::new(),
            markers: Vec::new(),
            shutter_angle: 180.0,
            shutter_phase: -90.0,
            motion_blur_samples: 16,
            motion_blur_adaptive_limit: 128,
            renderer: Default::default(),
            hide_shy: false,
            enable_motion_blur: true,
            enable_frame_blending: true,
            draft_3d: false,
            preserve_frame_rate: false,
            preserve_resolution: false,
            poster_time: Tick::ZERO,
            global_light: Default::default(),
            guides: Vec::new(),
            essential: None,
        };

        for dlayer in dcomp.layers {
            let source = match dlayer.layer_type.as_slice() {
                b"SLay" => {
                    let sid = ItemId(project.alloc());
                    let s = Solid {
                        color: [0.5, 0.5, 0.5],
                        width: dcomp.width,
                        height: dcomp.height,
                        pixel_aspect: 1.0,
                    };
                    project.items.insert(
                        sid,
                        Item {
                            id: sid,
                            name: dlayer.name.clone(),
                            label: Label::Red,
                            comment: String::new(),
                            parent: Some(solids_folder),
                            kind: ItemKind::Solid(s),
                            proxy: None,
                        },
                    );
                    LayerSource::Solid { item: sid }
                }
                b"CLay" => LayerSource::Camera,
                _ if dlayer.is_text => LayerSource::Text,
                _ if dlayer.is_shape => LayerSource::Shape,
                _ => {
                    if let Some(&target_ftg) = footage_ids.get(&dlayer.source_item_id) {
                        LayerSource::Footage { item: target_ftg }
                    } else if let Some(&target_comp) = comp_by_raw_id.get(&dlayer.source_item_id).filter(|&&id| id != cid) {
                        LayerSource::Comp { item: target_comp }
                    } else if let Some(&target_ftg) = footage_by_name.get(&dlayer.name) {
                        LayerSource::Footage { item: target_ftg }
                    } else if let Some(&target_comp) = comp_ids.get(&dlayer.name).filter(|&&id| id != cid) {
                        LayerSource::Comp { item: target_comp }
                    } else {
                        LayerSource::Null
                    }
                }
            };

            let mut layer = build::layer(
                &mut project,
                &comp,
                &dlayer.name,
                source,
                (dcomp.width, dcomp.height),
                Some(duration),
            );

            layer.start_time = Tick::from_seconds_f64(dlayer.start_sec);
            layer.in_point = Tick::from_seconds_f64(dlayer.in_sec);
            layer.out_point = Tick::from_seconds_f64(dlayer.out_sec.max(dlayer.in_sec + 0.033));
            layer.switches.three_d = dlayer.three_d || layer.is_camera();
            layer.switches.motion_blur = true;

            if dlayer.is_text {
                let font_name = dlayer.text_font.as_deref()
                    .or_else(|| fonts.first().map(String::as_str))
                    .unwrap_or("SegoeUI")
                    .to_string();
                let doc = effectcraft_keyframe::TextDoc {
                    text: dlayer.name.clone(),
                    font: font_name,
                    size: 48.0,
                    fill: [1.0, 1.0, 1.0, 1.0],
                    apply_fill: true,
                    ..Default::default()
                };
                if let Some(pr) = layer.props.prop_mut("text/sourceText") {
                    pr.value = effectcraft_keyframe::Value::Text(Box::new(doc));
                }
            }

            comp.layers.push(layer);
        }

        let parent = dcomp.parent_folder.as_deref().and_then(|p| folder_ids.get(p)).copied();
        project.items.insert(
            cid,
            Item {
                id: cid,
                name: dcomp.name,
                label: Label::Sandstone,
                comment: String::new(),
                parent,
                kind: ItemKind::Comp(Arc::new(comp)),
                proxy: None,
            },
        );
    }

    let missing_count = missing_footage.len();
    let report = AepReport {
        project_path: project_path.map(|p| p.to_string_lossy().to_string()),
        fonts,
        missing_fonts: Vec::new(),
        missing_footage,
        total_footage_count: total_footage,
        missing_footage_count: missing_count,
        comp_count,
        plugins,
        custom_plugins,
    };

    Ok((project, report))
}


/// Decode an After Effects XML project (`.aepx`) into an [`effectcraft_project::Project`].
pub fn from_aepx(bytes: &[u8]) -> Result<Project, String> {
    let s = std::str::from_utf8(bytes).map_err(|e| format!("invalid UTF-8 in AEPX: {e}"))?;
    let t = s.trim_start_matches('\u{feff}').trim_start();
    if !is_aepx(bytes) && !t.starts_with("<?xml") {
        return Err("not an After Effects XML project".into());
    }

    let mut project = Project::default();
    let comp_w = extract_xml_num(t, "width").unwrap_or(1920) as u32;
    let comp_h = extract_xml_num(t, "height").unwrap_or(1080) as u32;
    let fps = extract_xml_num(t, "frameRate").unwrap_or(30) as f64;
    let dur_secs = extract_xml_num(t, "duration").unwrap_or(10) as f64;

    let frame_rate = FrameRate::from_f64(fps);
    let duration = Tick((dur_secs * 1_000_000.0) as i64);

    let comp = Comp {
        width: comp_w,
        height: comp_h,
        pixel_aspect: 1.0,
        frame_rate,
        duration,
        display_start: Tick::ZERO,
        background: [0.0, 0.0, 0.0],
        work_area: (Tick::ZERO, duration),
        layers: Vec::new(),
        markers: Vec::new(),
        shutter_angle: 180.0,
        shutter_phase: -90.0,
        motion_blur_samples: 16,
        motion_blur_adaptive_limit: 128,
        renderer: Default::default(),
        hide_shy: false,
        enable_motion_blur: true,
        enable_frame_blending: true,
        draft_3d: false,
        preserve_frame_rate: false,
        preserve_resolution: false,
        poster_time: Tick::ZERO,
        global_light: Default::default(),
        guides: Vec::new(),
        essential: None,
    };

    let cid = ItemId(project.alloc());
    project.items.insert(
        cid,
        Item {
            id: cid,
            name: "Composition 1".into(),
            label: Label::Sandstone,
            comment: String::new(),
            parent: None,
            kind: ItemKind::Comp(Arc::new(comp)),
            proxy: None,
        },
    );

    Ok(project)
}

fn extract_xml_num(xml: &str, tag: &str) -> Option<i64> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let Some(start) = xml.find(&open) {
        let val_start = start + open.len();
        if let Some(end) = xml[val_start..].find(&close) {
            let text = xml[val_start..val_start + end].trim();
            return text.parse::<f64>().ok().map(|v| v as i64);
        }
    }
    None
}

/// Decode an After Effects project from either binary `.aep` or XML `.aepx` format.
pub fn from_aep_or_aepx(bytes: &[u8]) -> Result<Project, String> {
    from_aep_or_aepx_with_report(bytes, None).map(|(p, _)| p)
}

/// Decode an After Effects project with a detailed report on assets, fonts, and plugins.
pub fn from_aep_or_aepx_with_report(bytes: &[u8], project_path: Option<&std::path::Path>) -> Result<(Project, AepReport), String> {
    if is_aep(bytes) {
        from_aep_with_report(bytes, project_path)
    } else if is_aepx(bytes) {
        let p = from_aepx(bytes)?;
        let mut report = AepReport::default();
        report.comp_count = p.comps().count();
        report.project_path = project_path.map(|p| p.to_string_lossy().to_string());
        Ok((p, report))
    } else {
        Err("unrecognized After Effects project format (not RIFX or AEPX)".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_aep_signature() {
        let mut sample = Vec::new();
        sample.extend_from_slice(b"RIFX");
        sample.extend_from_slice(&100u32.to_be_bytes());
        sample.extend_from_slice(b"Egg!");
        assert!(is_aep(&sample));

        sample[0..4].copy_from_slice(b"RIFF");
        assert!(is_aep(&sample));

        sample[8..12].copy_from_slice(b"WAVE");
        assert!(!is_aep(&sample));
    }

    #[test]
    fn parses_saas_promo_aep_and_relinks_footage() {
        let path = r"C:\Users\Moses\Desktop\After effects projects\ai-saas-promo-62650161\SaaSPromo\SaaSPromo.aep";
        if let Ok(bytes) = std::fs::read(path) {
            let (proj, report) = from_aep_or_aepx_with_report(&bytes, Some(std::path::Path::new(path))).expect("decodes SaaSPromo AEP");
            assert!(proj.comps().count() >= 5, "has compositions");
            assert!(!report.plugins.is_empty(), "detects plugins");
            assert!(!report.custom_plugins.is_empty(), "detects CC plugins as custom/third-party");
            assert!(!report.fonts.is_empty(), "detects fonts");
            assert_eq!(report.missing_footage_count, 0, "intelligently relinks local footage next to project");
        }
    }

    #[test]
    fn parses_real_aep_file() {
        let path = r"C:\Users\Moses\Downloads\MGPack v2\Shock pack\01_Instagram Stories\Instagram_stories_1.aep";
        if let Ok(bytes) = std::fs::read(path) {
            let proj = from_aep(&bytes).ok().expect("decodes AEP project");
            assert!(proj.comps().count() >= 3, "has at least 3 comps");
            let main_comp = proj.items.values().find(|it| it.name == "Instagram Stories 1").expect("main comp exists");
            let c = main_comp.as_comp().expect("is comp");
            assert_eq!(c.width, 1080);
            assert_eq!(c.height, 1920);
            assert_eq!(c.layers.len(), 24);
        }
    }

    #[test]
    fn parses_untitled_project_aep_with_em_sources() {
        let path = r"C:\Users\Moses\Desktop\After effects projects\Editing MarketPlace\Untitled Project.aep";
        if let Ok(bytes) = std::fs::read(path) {
            let (proj, report) = from_aep_or_aepx_with_report(&bytes).ok().expect("decodes Untitled Project AEP");
            assert!(proj.comps().count() >= 2, "has at least 2 comps (Comp 1 and MAIN)");

            // Verify EM_SOURCES folder exists and has child items
            let em_folder = proj.items.values().find(|it| it.name == "EM_SOURCES").expect("EM_SOURCES folder exists");
            assert!(em_folder.is_folder());
            let kids = proj.children(Some(em_folder.id));
            assert!(!kids.is_empty(), "EM_SOURCES has children");
            assert!(kids.len() >= 20, "EM_SOURCES has footage items");

            // Verify MAIN comp frame rate is 30.0 fps (not 1.0 fps!)
            let main_comp = proj.items.values().find(|it| it.name == "MAIN").expect("MAIN comp exists");
            let c = main_comp.as_comp().expect("is comp");
            assert_eq!(c.frame_rate.as_f64(), 30.0, "frame rate is 30.0");
            assert!(c.layers.len() >= 300, "has over 300 layers");

            // Verify layer timing is within reasonable bounds
            let first_layer = &c.layers[0];
            let in_sec = first_layer.in_point.as_seconds_f64();
            assert!(in_sec < 60.0, "layer in_point is reasonable ({in_sec}s)");

            // Verify fonts were detected
            assert!(!report.fonts.is_empty(), "detected fonts in project");
        }
    }

    #[test]
    fn session_file_open_and_import_aep() {
        let path = r"C:\Users\Moses\Downloads\MGPack v2\Shock pack\01_Instagram Stories\Instagram_stories_1.aep";
        if std::path::Path::new(path).exists() {
            let mut s = crate::Session::default();
            let res = s.execute("file.open", serde_json::json!({"path": path})).expect("file.open succeeds on AEP");
            assert_eq!(res["type"], "aep");
            assert!(res["comps"].as_u64().unwrap_or(0) >= 3);
            assert!(s.active_comp_id().is_some());

            // Test file.import into an existing session
            let mut s2 = crate::Session::default();
            s2.execute("comp.new", serde_json::json!({"name": "CurrentComp"})).expect("comp.new");
            let imp_res = s2.execute("file.import", serde_json::json!({"paths": [path]})).expect("file.import succeeds on AEP");
            assert!(!imp_res["comps"].as_array().unwrap().is_empty());
        }
    }
}


