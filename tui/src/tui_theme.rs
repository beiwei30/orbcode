use std::sync::{OnceLock, RwLock};

use orbcode_config::ThemeSetting;
use ratatui::prelude::{Color, Modifier, Style};

pub(crate) const USER_BAR_BG: Color = Color::Rgb(55, 55, 55);
pub(crate) const CLAUDE_ORANGE: Color = Color::Rgb(215, 119, 87);
const CLAUDE_PINK: Color = Color::Rgb(232, 134, 146);
pub(crate) const TOOL_BLUE: Color = Color::Rgb(122, 180, 232);
pub(crate) const SUCCESS_GREEN: Color = Color::Rgb(78, 186, 101);
pub(crate) const ERROR_PINK: Color = Color::Rgb(255, 107, 128);
const WARNING_YELLOW: Color = Color::Rgb(255, 193, 7);
pub(crate) const DIFF_ADDED_BG: Color = Color::Rgb(8, 68, 48);
pub(crate) const DIFF_REMOVED_BG: Color = Color::Rgb(86, 36, 44);
const DEFAULT_TUI_PALETTE: TuiPalette = TuiPalette {
    text: None,
    inverse_text: Color::White,
    subtle: None,
    user_bar_bg: USER_BAR_BG,
    claude: CLAUDE_ORANGE,
    claude_alt: CLAUDE_PINK,
    tool: TOOL_BLUE,
    success: SUCCESS_GREEN,
    error: ERROR_PINK,
    warning: WARNING_YELLOW,
    diff_added_bg: DIFF_ADDED_BG,
    diff_removed_bg: DIFF_REMOVED_BG,
    list_marker: Color::LightBlue,
    accent: Color::Cyan,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TuiPalette {
    pub(crate) text: Option<Color>,
    pub(crate) inverse_text: Color,
    pub(crate) subtle: Option<Color>,
    pub(crate) user_bar_bg: Color,
    pub(crate) claude: Color,
    pub(crate) claude_alt: Color,
    pub(crate) tool: Color,
    pub(crate) success: Color,
    pub(crate) error: Color,
    pub(crate) warning: Color,
    pub(crate) diff_added_bg: Color,
    pub(crate) diff_removed_bg: Color,
    pub(crate) list_marker: Color,
    pub(crate) accent: Color,
}

static ACTIVE_TUI_PALETTE: OnceLock<RwLock<TuiPalette>> = OnceLock::new();

fn active_palette_lock() -> &'static RwLock<TuiPalette> {
    ACTIVE_TUI_PALETTE.get_or_init(|| RwLock::new(DEFAULT_TUI_PALETTE))
}

pub(crate) fn active_palette() -> TuiPalette {
    match active_palette_lock().read() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

pub(crate) fn set_active_theme(theme: ThemeSetting) {
    let mut guard = match active_palette_lock().write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = palette_for_theme(theme);
}

pub(crate) fn palette_for_theme(theme: ThemeSetting) -> TuiPalette {
    match theme {
        ThemeSetting::Auto | ThemeSetting::Dark => DEFAULT_TUI_PALETTE,
        ThemeSetting::Light => TuiPalette {
            text: Some(Color::Black),
            inverse_text: Color::White,
            subtle: Some(Color::Rgb(102, 102, 102)),
            user_bar_bg: Color::Rgb(240, 240, 240),
            claude: Color::Rgb(215, 119, 87),
            claude_alt: Color::Rgb(215, 119, 87),
            tool: Color::Rgb(87, 105, 247),
            success: Color::Rgb(44, 122, 57),
            error: Color::Rgb(171, 43, 63),
            warning: Color::Rgb(150, 108, 30),
            diff_added_bg: Color::Rgb(199, 225, 203),
            diff_removed_bg: Color::Rgb(253, 210, 216),
            list_marker: Color::Rgb(87, 105, 247),
            accent: Color::Rgb(0, 130, 130),
        },
        ThemeSetting::DarkDaltonized => TuiPalette {
            claude: Color::Rgb(122, 180, 232),
            claude_alt: Color::Rgb(122, 180, 232),
            tool: Color::Rgb(147, 197, 253),
            success: Color::Rgb(45, 212, 191),
            error: Color::Rgb(251, 146, 60),
            warning: Color::Rgb(250, 204, 21),
            diff_added_bg: Color::Rgb(21, 68, 78),
            diff_removed_bg: Color::Rgb(82, 55, 18),
            list_marker: Color::Rgb(147, 197, 253),
            ..DEFAULT_TUI_PALETTE
        },
        ThemeSetting::LightDaltonized => TuiPalette {
            text: Some(Color::Black),
            inverse_text: Color::White,
            subtle: Some(Color::Rgb(102, 102, 102)),
            user_bar_bg: Color::Rgb(240, 240, 240),
            claude: Color::Rgb(37, 99, 235),
            claude_alt: Color::Rgb(37, 99, 235),
            tool: Color::Rgb(37, 99, 235),
            success: Color::Rgb(8, 145, 178),
            error: Color::Rgb(202, 138, 4),
            warning: Color::Rgb(150, 108, 30),
            diff_added_bg: Color::Rgb(202, 232, 240),
            diff_removed_bg: Color::Rgb(245, 226, 179),
            list_marker: Color::Rgb(37, 99, 235),
            accent: Color::Rgb(0, 130, 130),
        },
        ThemeSetting::DarkAnsi => TuiPalette {
            claude: Color::Red,
            claude_alt: Color::Magenta,
            tool: Color::Blue,
            success: Color::Green,
            error: Color::Red,
            warning: Color::Yellow,
            diff_added_bg: Color::DarkGray,
            diff_removed_bg: Color::DarkGray,
            list_marker: Color::Blue,
            ..DEFAULT_TUI_PALETTE
        },
        ThemeSetting::LightAnsi => TuiPalette {
            text: Some(Color::Black),
            inverse_text: Color::White,
            subtle: Some(Color::DarkGray),
            user_bar_bg: Color::White,
            claude: Color::Red,
            claude_alt: Color::Red,
            tool: Color::Blue,
            success: Color::Green,
            error: Color::Red,
            warning: Color::Yellow,
            diff_added_bg: Color::Green,
            diff_removed_bg: Color::Red,
            list_marker: Color::Blue,
            accent: Color::Cyan,
        },
    }
}

pub(crate) fn theme_label(theme: ThemeSetting) -> &'static str {
    match theme {
        ThemeSetting::Auto => "Auto (match terminal)",
        ThemeSetting::Dark => "Dark mode",
        ThemeSetting::Light => "Light mode",
        ThemeSetting::DarkDaltonized => "Dark mode (colorblind-friendly)",
        ThemeSetting::LightDaltonized => "Light mode (colorblind-friendly)",
        ThemeSetting::DarkAnsi => "Dark mode (ANSI colors only)",
        ThemeSetting::LightAnsi => "Light mode (ANSI colors only)",
    }
}

pub(crate) fn output_style_label(style: &str) -> &str {
    if style == "default" { "Default" } else { style }
}

pub(crate) fn subtle_style() -> Style {
    match active_palette().subtle {
        Some(color) => Style::default().fg(color),
        None => Style::default().add_modifier(Modifier::DIM),
    }
}

pub(crate) fn inactive_style() -> Style {
    match active_palette().text {
        Some(color) => Style::default().fg(color),
        None => Style::default(),
    }
}

pub(crate) fn empty_transcript_placeholder_style() -> Style {
    subtle_style()
}

pub(crate) fn warning_style() -> Style {
    Style::default().fg(active_palette().warning)
}

pub(crate) fn heading_style(level: usize) -> Style {
    let palette = active_palette();
    let color = match level {
        1 => palette.claude,
        2 => palette.warning,
        _ => palette.accent,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

pub(crate) fn emphasis_fg() -> Color {
    let palette = active_palette();
    palette.text.unwrap_or(palette.inverse_text)
}

pub(crate) fn emphasis_style() -> Style {
    Style::default().fg(emphasis_fg())
}

pub(crate) fn highlight_style() -> Style {
    emphasis_style().add_modifier(Modifier::BOLD)
}

pub(crate) fn accent_style() -> Style {
    Style::default().fg(active_palette().accent)
}

pub(crate) fn quote_style() -> Style {
    Style::default().fg(active_palette().success)
}

pub(crate) fn list_marker_style() -> Style {
    Style::default()
        .fg(active_palette().list_marker)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn code_block_style() -> Style {
    Style::default().fg(active_palette().tool)
}

pub(crate) fn inline_markdown_style(base_style: Style, bold: bool, code: bool) -> Style {
    let mut style = base_style;
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if code {
        style = style.fg(active_palette().tool);
    }
    style
}

pub(crate) fn user_bar_style() -> Style {
    let palette = active_palette();
    Style::default()
        .fg(palette.inverse_text)
        .bg(palette.user_bar_bg)
}

pub(crate) fn prompt_input_style() -> Style {
    inactive_style()
}

pub(crate) fn stats_heatmap_color(index: usize) -> Color {
    let palette = active_palette();
    if palette.text.is_some() {
        return light_heatmap_color(index);
    }
    if palette.subtle.is_none() {
        return dark_heatmap_color(index);
    }
    ansi_heatmap_color(index)
}

fn dark_heatmap_color(index: usize) -> Color {
    const BACKGROUND_RGB: (u8, u8, u8) = (0, 49, 58);
    const RELATIVE_STOPS: [(f64, f64, f64); 5] = [
        (4.326_019, 0.488_889, 0.062_745),
        (4.900_181, 0.311_475, 0.125_490),
        (1.800_766, 0.134_328, 0.280_392),
        (-5.780_933, 0.074_236, 0.437_255),
        (-9.310_345, 0.070_707, 0.498_039),
    ];
    let (base_hue, base_saturation, base_lightness) =
        rgb_to_hsl(BACKGROUND_RGB.0, BACKGROUND_RGB.1, BACKGROUND_RGB.2);
    let (hue_offset, saturation_multiplier, lightness_offset) =
        RELATIVE_STOPS[index.min(RELATIVE_STOPS.len() - 1)];
    let (red, green, blue) = hsl_to_rgb(
        base_hue + hue_offset,
        base_saturation * saturation_multiplier,
        base_lightness + lightness_offset,
    );
    Color::Rgb(red, green, blue)
}

fn light_heatmap_color(index: usize) -> Color {
    const STOPS: [Color; 5] = [
        Color::Rgb(190, 220, 200),
        Color::Rgb(140, 195, 155),
        Color::Rgb(80, 160, 110),
        Color::Rgb(30, 130, 75),
        Color::Rgb(0, 100, 50),
    ];
    STOPS[index.min(STOPS.len() - 1)]
}

fn ansi_heatmap_color(index: usize) -> Color {
    const STOPS: [Color; 5] = [
        Color::DarkGray,
        Color::Green,
        Color::Cyan,
        Color::LightGreen,
        Color::LightCyan,
    ];
    STOPS[index.min(STOPS.len() - 1)]
}

fn rgb_to_hsl(red: u8, green: u8, blue: u8) -> (f64, f64, f64) {
    let red = red as f64 / 255.0;
    let green = green as f64 / 255.0;
    let blue = blue as f64 / 255.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let lightness = (max + min) / 2.0;
    if (max - min).abs() < f64::EPSILON {
        return (0.0, 0.0, lightness);
    }

    let delta = max - min;
    let saturation = if lightness > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if (max - red).abs() < f64::EPSILON {
        60.0 * (((green - blue) / delta) + if green < blue { 6.0 } else { 0.0 })
    } else if (max - green).abs() < f64::EPSILON {
        60.0 * (((blue - red) / delta) + 2.0)
    } else {
        60.0 * (((red - green) / delta) + 4.0)
    };

    (hue, saturation, lightness)
}

fn hsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> (u8, u8, u8) {
    let hue = hue.rem_euclid(360.0);
    let saturation = saturation.clamp(0.0, 1.0);
    let lightness = lightness.clamp(0.0, 1.0);
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let x = chroma * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = lightness - chroma / 2.0;
    let (red, green, blue) = if hue < 60.0 {
        (chroma, x, 0.0)
    } else if hue < 120.0 {
        (x, chroma, 0.0)
    } else if hue < 180.0 {
        (0.0, chroma, x)
    } else if hue < 240.0 {
        (0.0, x, chroma)
    } else if hue < 300.0 {
        (x, 0.0, chroma)
    } else {
        (chroma, 0.0, x)
    };

    (
        ((red + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((green + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((blue + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}
