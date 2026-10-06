//! Shared application theme for every human-facing Agentlog surface.

use urushi::{Color, ColorScheme, SemanticTokens, Theme, ThemeSet};

pub(crate) const DEFAULT_COLOR_SCHEME: ColorScheme = ColorScheme::Dark;

pub(crate) fn theme_set() -> ThemeSet {
    ThemeSet::new(
        theme(SemanticTokens {
            text: Color::Rgb(35, 39, 47),
            text_muted: Color::Rgb(91, 99, 112),
            background: Color::Rgb(248, 249, 251),
            surface: Color::Rgb(235, 238, 243),
            accent: Color::Rgb(30, 105, 210),
            accent_text: Color::Rgb(255, 255, 255),
            success: Color::Rgb(26, 127, 55),
            warning: Color::Rgb(154, 103, 0),
            error: Color::Rgb(207, 34, 46),
            border: Color::Rgb(140, 149, 159),
        }),
        theme(SemanticTokens {
            text: Color::Rgb(230, 230, 230),
            text_muted: Color::Rgb(150, 150, 150),
            background: Color::Rgb(20, 24, 28),
            surface: Color::Rgb(32, 36, 44),
            accent: Color::Rgb(100, 170, 255),
            accent_text: Color::Rgb(20, 24, 28),
            success: Color::Rgb(80, 200, 120),
            warning: Color::Rgb(240, 190, 70),
            error: Color::Rgb(255, 100, 100),
            border: Color::Rgb(110, 120, 130),
        }),
    )
}

fn theme(tokens: SemanticTokens) -> Theme {
    Theme::from_tokens(tokens)
}

#[cfg(test)]
mod tests {
    use urushi::ComponentRole;

    use super::{DEFAULT_COLOR_SCHEME, theme_set};

    #[test]
    fn both_schemes_share_the_same_component_vocabulary() {
        let themes = theme_set();
        let selected = themes.select(DEFAULT_COLOR_SCHEME);

        assert_eq!(
            selected.text_style(ComponentRole::Body),
            themes.dark().text_style(ComponentRole::Body)
        );
        assert_ne!(
            themes.light().tokens().background,
            themes.dark().tokens().background
        );
    }
}
