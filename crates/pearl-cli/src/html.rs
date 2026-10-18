//! Render the dashboard into a ratatui TestBackend and write the cell grid as a
//! colored HTML page (and plain text), for screenshots with a headless browser.

use anyhow::Result;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

use crate::snapshot::Snapshot;
use crate::ui::{self, App, Tab};

fn hex(c: Color, default: &str) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Black => "#000000".into(),
        Color::White => "#ffffff".into(),
        Color::Reset => default.into(),
        _ => default.into(),
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn buffer_text(b: &Buffer) -> String {
    (0..b.area.height).map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n")
}

/// One `<pre>` with a span per run of equal style.
pub fn buffer_html(b: &Buffer, title: &str) -> String {
    let mut body = String::new();
    for y in 0..b.area.height {
        let mut x = 0;
        while x < b.area.width {
            let c = &b[(x, y)];
            let (fg, bg, bold) = (c.fg, c.bg, c.modifier.contains(Modifier::BOLD));
            let mut run = String::new();
            while x < b.area.width {
                let d = &b[(x, y)];
                if d.fg != fg || d.bg != bg || d.modifier.contains(Modifier::BOLD) != bold {
                    break;
                }
                run.push_str(d.symbol());
                x += 1;
            }
            body.push_str(&format!(
                "<span style=\"color:{};background:{}{}\">{}</span>",
                hex(fg, "#ffffff"),
                hex(bg, "#0000aa"),
                if bold { ";font-weight:bold" } else { "" },
                esc(&run)
            ));
        }
        body.push('\n');
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title><style>\
         html,body{{margin:0;background:#0000aa}}\
         pre{{margin:0;padding:12px;font-family:Menlo,Monaco,'Courier New',monospace;font-size:14px;line-height:17px;\
         letter-spacing:0;white-space:pre;display:inline-block}}\
         pre span{{display:inline-block;height:17px;vertical-align:top}}\
         </style></head><body><pre>{}</pre></body></html>",
        esc(title),
        body
    )
}

pub fn render_app(app: &App, w: u16, h: u16) -> Result<Buffer> {
    let mut t = Terminal::new(TestBackend::new(w, h))?;
    t.draw(|f| ui::render(f, app))?;
    Ok(t.backend().buffer().clone())
}

pub fn render_snapshot(sn: &Snapshot, w: u16, h: u16, tab: Tab) -> Result<(String, String)> {
    let mut app = App::new(sn.clone(), sn.me.is_some());
    app.set_tab(tab);
    app.log.push((false, format!("snapshot of the live stack, {} errors", sn.errors.len())));
    let b = render_app(&app, w, h)?;
    Ok((buffer_html(&b, &format!("{} dashboard", crate::brand::NAME)), buffer_text(&b)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::sample;

    #[test]
    fn html_has_colors_and_escapes() {
        let (html, text) = render_snapshot(&sample::snapshot(), 140, 46, Tab::Jobs).unwrap();
        assert!(html.contains("background:#0000aa"));
        assert!(html.contains("background:#55ff55"), "selected row");
        assert!(html.contains("color:#ffff55"), "yellow titles");
        assert!(html.contains(&esc(&crate::brand::bracket_mark())));
        assert_eq!(text.lines().count(), 46);
        assert!(text.contains("/Wallet Balance"));
    }

    /// `PEARL_SAMPLE_OUT=dir cargo test sample_out` writes the sample screen (HTML + text).
    #[test]
    fn sample_out() {
        let Ok(dir) = std::env::var("PEARL_SAMPLE_OUT") else { return };
        let (html, text) = render_snapshot(&sample::snapshot(), 140, 46, Tab::Jobs).unwrap();
        std::fs::write(format!("{dir}/sample.html"), html).unwrap();
        std::fs::write(format!("{dir}/sample.txt"), text).unwrap();
    }
}
