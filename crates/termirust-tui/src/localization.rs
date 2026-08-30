#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TuiLocale {
    English,
    PseudoExpanded,
    PseudoRtl,
}

impl TuiLocale {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "en-US" | "en" => Some(Self::English),
            "en-XA" => Some(Self::PseudoExpanded),
            "ar-XB" => Some(Self::PseudoRtl),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextId {
    AppTitle,
    Projects,
    Sessions,
    Inspector,
    AllSessions,
    Empty,
    Loading,
    Partial,
    RecoveryRequired,
    Unavailable,
    Filter,
    HelpTitle,
    HelpKeys,
    SmallTerminal,
    ReadOnly,
}

pub fn text(locale: TuiLocale, id: TextId) -> String {
    let english = match id {
        TextId::AppTitle => "TermiRust Fleet",
        TextId::Projects => "Projects",
        TextId::Sessions => "Sessions",
        TextId::Inspector => "Inspector",
        TextId::AllSessions => "All sessions",
        TextId::Empty => "No Projects or Sessions",
        TextId::Loading => "Loading existing metadata...",
        TextId::Partial => "Some records were skipped",
        TextId::RecoveryRequired => "Recovery review required",
        TextId::Unavailable => "Metadata unavailable",
        TextId::Filter => "Filter",
        TextId::HelpTitle => "Keyboard help",
        TextId::HelpKeys => {
            "Arrows/j/k move  Tab changes pane  / filters  i inspector  r refresh  ? help  q quit"
        }
        TextId::SmallTerminal => "Terminal must be at least 80 columns by 20 rows",
        TextId::ReadOnly => "Local controller",
    };
    localize(locale, english)
}

pub fn localize(locale: TuiLocale, english: &str) -> String {
    match locale {
        TuiLocale::English => english.to_string(),
        TuiLocale::PseudoExpanded => {
            format!("[!! {} {} !!]", english, "~".repeat(english.len() / 2))
        }
        TuiLocale::PseudoRtl => format!("\u{2067}{}\u{2069}", english),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_locale_produces_visible_bounded_text() {
        for locale in [
            TuiLocale::English,
            TuiLocale::PseudoExpanded,
            TuiLocale::PseudoRtl,
        ] {
            for id in [
                TextId::AppTitle,
                TextId::Projects,
                TextId::Sessions,
                TextId::Inspector,
                TextId::AllSessions,
                TextId::Empty,
                TextId::Loading,
                TextId::Partial,
                TextId::RecoveryRequired,
                TextId::Unavailable,
                TextId::Filter,
                TextId::HelpTitle,
                TextId::HelpKeys,
                TextId::SmallTerminal,
                TextId::ReadOnly,
            ] {
                let rendered = text(locale, id);
                assert!(!rendered.is_empty());
                assert!(rendered.chars().count() <= 256);
            }
        }
    }
}
