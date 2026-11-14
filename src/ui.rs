use crate::app::{App, Screen};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};

fn short_url(url: &str) -> String {
    let base = url.split('?').next().unwrap_or(url);
    let base = base
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("pull-");
    if base.chars().count() > 66 {
        let tail: String = base
            .chars()
            .rev()
            .take(44)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("…{tail}")
    } else {
        base.to_string()
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);

    match app.screen {
        Screen::Input => draw_input(f, app, chunks[1]),
        Screen::Loading => draw_loading(f, app, chunks[1]),
        Screen::Quality => draw_quality(f, app, chunks[1]),
    }

    draw_footer(f, app, chunks[2]);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let line = if let Some(r) = &app.room {
        format!("▶ {} · {}   (在线 {})", r.nick, r.title, r.online)
    } else if app.screen == Screen::Loading {
        format!("{} {}", app.spinner_char(), app.msg)
    } else {
        "抖音直播直链 → mpv".to_string()
    };

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray));
    let p = Paragraph::new(Text::from(vec![
        Line::from(vec![
            Span::styled(
                " dy-live ",
                Style::default().fg(Color::Black).bg(Color::Red).bold(),
            ),
            Span::raw(" "),
            Span::styled("TUI", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(Span::styled(line, Style::default().fg(Color::White))),
    ]))
    .block(block);
    f.render_widget(p, area);
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let cursor = if app.input.is_empty() { "" } else { "" };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("直播间 ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "URL 或 房间号",
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("❯ ", Style::default().fg(Color::Red).bold()),
            Span::raw(app.input.clone()),
            Span::styled(format!("{cursor}█"), Style::default().fg(Color::Red)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "例：456497901213   或   https://live.douyin.com/456497901213",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    if let Some(err) = &app.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("✗ {err}"),
            Style::default().fg(Color::Red),
        )));
    }

    if !app.history.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "最近打开  ↑/↓ 选择，回车重新打开",
            Style::default().fg(Color::DarkGray),
        )));
        for (i, e) in app.history.entries().iter().take(5).enumerate() {
            let selected = app.hist_sel == Some(i);
            let style = if selected {
                Style::default().fg(Color::Black).bg(Color::Red)
            } else {
                Style::default().fg(Color::Gray)
            };
            let marker = if selected { "❯ " } else { "  " };
            lines.push(Line::from(Span::styled(
                format!("{marker}{:<12} {}", e.rid, e.summary(46)),
                style,
            )));
        }
    }

    let block = Block::default()
        .title(Span::styled(" 输入 ", Style::default().fg(Color::DarkGray)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_loading(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {} {}", app.spinner_char(), app.msg),
            Style::default().fg(Color::Yellow),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  解析房间 → 拉 sigend 直链 → 逐条探测 CDN",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(Paragraph::new(Text::from(text)).block(block), area);
}

fn draw_quality(f: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(vec![
        Cell::from("质量"),
        Cell::from("分辨率"),
        Cell::from("协议"),
        Cell::from("连通"),
        Cell::from("延迟"),
        Cell::from("直链"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = app
        .cands
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let pending = !c.probe.done;
            let ok = c.probe.ok();
            let (mark, color) = if pending {
                (format!("{}", app.spinner_char()), Color::Yellow)
            } else if ok {
                ("✓ 200".to_string(), Color::Green)
            } else if c.probe.code == 0 {
                ("✗ 超时".to_string(), Color::Red)
            } else {
                (format!("✗ {}", c.probe.code), Color::Red)
            };
            let latency = if pending {
                "-".to_string()
            } else {
                format!("{} ms", c.probe.ms)
            };
            let q = c.display_name();
            let res = if c.res.is_empty() {
                "-".to_string()
            } else if c.fps > 0 {
                format!("{}@{}", c.res, c.fps)
            } else {
                c.res.clone()
            };
            let style = if i == app.sel {
                Style::default().fg(Color::Black).bg(Color::Red)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(q),
                Cell::from(res).style(Style::default().fg(Color::Gray)),
                Cell::from(c.proto.as_str()),
                Cell::from(mark).style(Style::default().fg(color)),
                Cell::from(latency),
                Cell::from(short_url(&c.url)).style(Style::default().fg(Color::DarkGray)),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(12),
        Constraint::Length(14),
        Constraint::Length(4),
        Constraint::Length(7),
        Constraint::Length(8),
        Constraint::Min(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(
                    format!(" 清晰度 · {} 条 ", app.cands.len()),
                    Style::default().fg(Color::Gray),
                )),
        )
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol("");

    let mut state = ratatui::widgets::TableState::default();
    state.select(Some(app.sel));
    f.render_stateful_widget(table, area, &mut state);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let keys = match app.screen {
        Screen::Input => "Enter 解析   ↑/↓ 最近打开   Ctrl-U 清空   Esc 退出",
        Screen::Loading => "Esc 取消",
        Screen::Quality => {
            "↑/↓ 选择   Enter 播放   f 强制播放   p 重测   r 重解析   Esc 返回   q 退出"
        }
    };
    let mut lines = vec![Line::from(Span::styled(
        format!(" {keys}"),
        Style::default().fg(Color::DarkGray),
    ))];
    if !app.msg.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(" {}", app.msg),
            Style::default().fg(Color::Gray),
        )));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render(app: &App, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        let area = buf.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(&buf[(x, y)].symbol().to_string());
            }
            out.push('\n');
        }
        out
    }

    /// 去掉空白：CJK 是双宽字符，缓冲区里每个字后面会跟一个空单元
    fn compact(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn input_screen_shows_prompt() {
        let app = App::new(false, None);
        let raw = render(&app, 80, 16);
        let s = compact(&raw);
        assert!(s.contains("dy-live"), "brand missing:\n{raw}");
        assert!(s.contains("直播间"), "prompt missing:\n{raw}");
        assert!(s.contains("Enter解析"), "footer missing:\n{raw}");
    }

    #[test]
    fn input_screen_with_error() {
        let mut app = App::new(false, None);
        app.input = "abc".into();
        app.error = Some("房间号或链接不对".into());
        let raw = render(&app, 80, 16);
        assert!(
            compact(&raw).contains("房间号或链接不对"),
            "error not shown:\n{raw}"
        );
    }

    #[test]
    fn input_screen_shows_history() {
        let mut app = App::new(false, None);
        let path = std::env::temp_dir().join(format!("dy-ui-hist-{}.tsv", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut h = crate::history::History::load_from(&path);
        h.add("456497901213", "主播杨树", "全新3D僵尸毁灭工程");
        app.history = h;
        app.input = "456497901213".into();
        app.hist_sel = Some(0);

        let s = compact(&render(&app, 80, 20));
        assert!(s.contains("最近打开"), "history title missing");
        assert!(s.contains("主播杨树"), "history row missing");
        assert!(s.contains("❯"), "selected marker missing");
        let _ = std::fs::remove_file(&path);
    }
}
