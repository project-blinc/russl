//! The toy's window painted offscreen: the starter sketch draws in the preview.

use blinc_recorder::Headless;
use blinc_recorder::testing::ScreenshotExporter;
use blinc_theme::ColorScheme;

#[test]
fn the_preview_draws_the_starter_sketch() {
    if !Headless::available() {
        eprintln!("no GPU adapter: skipped");
        return;
    }
    let mut ui = Headless::new(1400, 860).unwrap();
    ui.theme(russl_toy::ui::theme_bundle(), ColorScheme::Dark);
    ui.load_font_data(russl_toy::ui::CODE_FONT.to_vec());
    ui.mount(russl_toy::ui::build_ui);
    ui.advance(500).settle();
    let frame = ui.snapshot().unwrap();
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    ScreenshotExporter::save_png(&frame, dir.join("russl_toy.png")).unwrap();
    // The WGSL tab, where the panel's tab strip puts it: the bound output block.
    ui.move_to(477.0, 626.0).mouse_down().mouse_up().settle();
    let wgsl = ui.snapshot().unwrap();
    ScreenshotExporter::save_png(&wgsl, dir.join("russl_toy_wgsl.png")).unwrap();
    // Closed, the panel leaves and the editor and preview take the room.
    ui.click("#toy-toggle-panel").settle();
    let collapsed = ui.snapshot().unwrap();
    ScreenshotExporter::save_png(&collapsed, dir.join("russl_toy_collapsed.png")).unwrap();
    // Closed, the inspector leaves and the side container folds to the rail.
    ui.click("#toy-toggle-inspector").settle();
    let folded = ui.snapshot().unwrap();
    ScreenshotExporter::save_png(&folded, dir.join("russl_toy_folded.png")).unwrap();
    eprintln!("saved {}", dir.display());
    // The preview fills the right half's top; a drawn sketch varies across it.
    let px = |x: u32, y: u32| {
        let i = ((y * frame.width + x) * 4) as usize;
        [frame.data[i], frame.data[i + 1], frame.data[i + 2]]
    };
    let (x0, y0) = (frame.width * 3 / 4, frame.height / 6);
    let samples: Vec<_> = (0..8).map(|k| px(x0 - 100 + k * 25, y0 + k * 10)).collect();
    assert!(
        samples.windows(2).any(|w| w[0] != w[1]),
        "the preview is flat: {samples:?}"
    );
}

#[test]
fn problems_show_inline_in_their_module_and_in_the_panel() {
    if !Headless::available() {
        return;
    }
    let mut ui = Headless::new(1400, 860).unwrap();
    ui.theme(russl_toy::ui::theme_bundle(), ColorScheme::Dark);
    ui.load_font_data(russl_toy::ui::CODE_FONT.to_vec());
    russl_toy::ui::load_sketch(&[
        russl_toy::sketch::Source {
            name: "main".into(),
            text: "use crate::shapes::ring;\n\n#[fragment]\n#[location(0)]\nfn fs_main(input: Frag) -> Vec4 {\n    let unused = 1.0;\n    Vec4::splat(ring(input.uv))\n}\n".into(),
        },
        russl_toy::sketch::Source {
            name: "shapes".into(),
            text: "pub fn ring(p: Vec2) -> f32 {\n    (p.length() - 0.3).abs() + true\n}\n".into(),
        },
    ]);
    ui.mount(russl_toy::ui::build_ui);
    ui.advance(300).settle();
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let frame = ui.snapshot().unwrap();
    ScreenshotExporter::save_png(&frame, dir.join("russl_toy_problems.png")).unwrap();
    assert!(
        ui.find(".blinc-code-lens-error").is_none(),
        "main has no error; shapes has it"
    );
    ui.click("#toy-status-problems").settle();
    let frame = ui.snapshot().unwrap();
    ScreenshotExporter::save_png(&frame, dir.join("russl_toy_problems_panel.png")).unwrap();
}
