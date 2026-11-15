use crate::app::{App, Screen};
use crate::discover;
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
        Screen::Discover => draw_discover(f, app, chunks[1]),
    }

    draw_footer(f, app, chunks[2]);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let line = if let Some(r) = &app.room {
        format!("▶ {} · {}   (在线 {})", r.nick, r.title, r.online)
    } else if app.screen == Screen::Loading {
        format!("{} {}", app.spinner_char(), app.msg)
    } else if app.screen == Screen::Discover {
        let cat = discover::CATEGORIES[app.discover.sel.min(discover::CATEGORIES.len() - 1)];
        format!("找直播 · {} 分区", cat.name)
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
            Span::styled("█", Style::default().fg(Color::Red)),
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
        let n = app.history.entries().len();
        let inner = area.height.saturating_sub(2) as usize;
        // 历史区上方已有 lines.len() 行，再加一行留白 + 一行标题
        let max_rows = inner.saturating_sub(lines.len() + 2).min(n);

        lines.push(Line::from(""));
        let marked = app.hist_marked.len();
        let title = if app.hist_nav {
            let mut t = format!("历史导航 {n} 条");
            if marked > 0 {
                t.push_str(&format!(" · 已选 {marked} 条"));
            }
            t.push_str("  j/k 移动 · x 标记 · d 删除 · Esc 返回");
            t
        } else {
            format!("最近打开 {n} 条  ↑/↓ 进入历史导航")
        };
        lines.push(Line::from(Span::styled(
            title,
            Style::default().fg(Color::DarkGray),
        )));

        // 以当前选中项为中心滚动，避免条目多了以后看不到
        let start = match app.hist_sel {
            Some(sel) => sel.saturating_sub(max_rows / 2),
            None => 0,
        };
        let start = start.min(n.saturating_sub(max_rows));
        let inner_w = area.width.saturating_sub(2) as usize;

        for (i, e) in app
            .history
            .entries()
            .iter()
            .enumerate()
            .skip(start)
            .take(max_rows)
        {
            let selected = app.hist_sel == Some(i);
            let marked = app.hist_marked.contains(&e.rid);
            let style = if selected {
                Style::default().fg(Color::Black).bg(Color::Red)
            } else if marked {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Gray)
            };
            let cursor = if selected { "❯" } else { " " };
            let check = if marked { "[x]" } else { "[ ]" };
            let nick = if e.nick.is_empty() {
                "（无主播名）"
            } else {
                e.nick.as_str()
            };
            // 主播名放最前，房间号固定保留，方便区分同名/同标题的条目
            let mut text = format!("{cursor}{check} {nick}  {}  {}", e.rid, e.title);
            if inner_w > 1 && text.chars().count() > inner_w {
                text = text.chars().take(inner_w - 1).collect::<String>();
                text.push('…');
            }
            lines.push(Line::from(Span::styled(text, style)));
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
            "  解析房间 → 拉 signed 直链 → 逐条探测 CDN",
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
                (app.spinner_char().to_string(), Color::Yellow)
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

fn draw_discover(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3)])
        .split(area);

    // 分区栏：选中的一块红底 + ▸，不依赖终端是否渲染 bold
    let mut tabs = Vec::with_capacity(discover::CATEGORIES.len() * 2);
    for (i, cat) in discover::CATEGORIES.iter().enumerate() {
        let selected = i == app.discover.sel;
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let label = if selected {
            format!("▸ {} ", cat.name)
        } else {
            format!("  {} ", cat.name)
        };
        tabs.push(Span::styled(label, style));
        tabs.push(Span::raw(" "));
    }
    let mut lines = vec![Line::from(tabs)];

    // 游戏分区的子分区（综合 + 9 个），小主播更多
    if app.discover.sel == 2 {
        let mut sub_spans = Vec::new();
        for (i, name) in std::iter::once("综合")
            .chain(discover::GAME_SUBS.iter().map(|s| s.name))
            .enumerate()
        {
            let selected = i == app.discover.sub;
            let style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let label = if selected {
                format!("▸ {} ", name)
            } else {
                format!("  {} ", name)
            };
            sub_spans.push(Span::styled(label, style));
        }
        lines.push(Line::from(sub_spans));
    } else {
        lines.push(Line::from(""));
    }

    f.render_widget(
        Paragraph::new(Text::from(lines)).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        chunks[0],
    );

    let header = Row::new(vec![
        Cell::from("主播"),
        Cell::from("标题"),
        Cell::from("在线"),
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    let mut rows: Vec<Row> = Vec::new();
    if app.discover.loading && app.discover.rooms.is_empty() {
        rows.push(
            Row::new(vec![
                Cell::from(format!("{} 加载中…", app.spinner_char())),
                Cell::from(""),
                Cell::from(""),
            ])
            .style(Style::default().fg(Color::Yellow)),
        );
    } else if let Some(err) = &app.discover.err {
        rows.push(
            Row::new(vec![
                Cell::from(format!("✗ {err}")),
                Cell::from(""),
                Cell::from(""),
            ])
            .style(Style::default().fg(Color::Red)),
        );
    } else if app.discover.rooms.is_empty() {
        rows.push(
            Row::new(vec![
                Cell::from("这个分区暂时没房间"),
                Cell::from("r 下一批"),
                Cell::from(""),
            ])
            .style(Style::default().fg(Color::DarkGray)),
        );
    }

    for (i, room) in app.discover.rooms.iter().enumerate() {
        let selected = i == app.discover.cursor;
        if selected {
            // 整行反白 + ▶ 前缀：只加 bold 在很多终端上根本看不出选中
            rows.push(
                Row::new(vec![
                    Cell::from(format!("▶ {}", room.nick)),
                    Cell::from(room.title.clone()),
                    Cell::from(room.online.clone()),
                ])
                .style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Red)
                        .add_modifier(Modifier::BOLD),
                ),
            );
        } else {
            rows.push(Row::new(vec![
                Cell::from(format!("  {}", room.nick)),
                Cell::from(room.title.clone()).style(Style::default().fg(Color::Gray)),
                Cell::from(room.online.clone()).style(Style::default().fg(Color::DarkGray)),
            ]));
        }
    }

    let order = if app.discover.sort_asc {
        " · 升序"
    } else {
        " · 降序"
    };
    let done = if app.discover.exhausted {
        " · 已看完"
    } else {
        ""
    };
    let fallback = if app.discover.fallback {
        " · 热榜回退"
    } else {
        ""
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(18),
            Constraint::Min(20),
            Constraint::Length(7),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                format!(
                    " 找直播 · {} · {} 个{order}{done}{fallback} ",
                    app.discover_label(),
                    app.discover.rooms.len()
                ),
                Style::default().fg(Color::Gray),
            )),
    )
    .highlight_symbol("");

    let mut state = ratatui::widgets::TableState::default();
    state.select(Some(app.discover.cursor));
    f.render_stateful_widget(table, chunks[1], &mut state);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let keys = match app.screen {
        Screen::Input if app.hist_nav => {
            "j/k 移动   Space/x 标记   V 全选   d 删除   Enter 打开   Esc 返回输入"
        }
        Screen::Input => "Enter 解析   Tab 找直播   ↑/↓ 历史   Ctrl-U 清空   Esc 退出",
        Screen::Loading => "Esc 取消",
        Screen::Quality => {
            "↑/↓ 选择   Enter 播放   f 强制播放   p 重测   r 重解析   Esc 返回   q 退出"
        }
        Screen::Discover => {
            "←/→ 分区   [ ] 子分区   ↑/↓ 或 j/k 选择   s 排序   r 下一批   R 重来   Enter 打开   Esc 返回"
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

    fn buffer(app: &App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        term.backend().buffer().clone()
    }

    fn render(app: &App, w: u16, h: u16) -> String {
        let buf = buffer(app, w, h);
        let area = buf.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buf[(x, y)].symbol());
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

        let raw = render(&app, 90, 20);
        let s = compact(&raw);
        assert!(s.contains("最近打开"), "history title missing");
        assert!(s.contains("主播杨树"), "history row missing");
        assert!(s.contains("456497901213"), "房间号没显示");
        assert!(raw.contains("[ ]"), "缺少多选标记框:\n{raw}");
        assert!(s.contains("❯"), "selected marker missing");
        assert!(s.contains("进入历史"), "缺少历史导航提示");
        assert!(s.contains("↑/↓"), "缺少历史导航按键提示");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn input_history_shows_marked_entries() {
        let mut app = App::new(false, None);
        let path = std::env::temp_dir().join(format!("dy-ui-hist-mark-{}.tsv", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut h = crate::history::History::load_from(&path);
        h.add("456497901213", "主播杨树", "全新3D僵尸毁灭工程");
        app.history = h;
        app.input = "456497901213".into();
        app.hist_sel = Some(0);
        app.hist_marked.insert("456497901213".into());

        let raw = render(&app, 90, 20);
        let s = compact(&raw);
        assert!(s.contains("[x]"), "已标记的条目要有勾:\n{raw}");
        assert!(s.contains("主播杨树"), "标记后主播名还要在:\n{raw}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn input_history_footer_switches_to_vim_keys() {
        let mut app = App::new(false, None);
        let path = std::env::temp_dir().join(format!("dy-ui-hist-vim-{}.tsv", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut h = crate::history::History::load_from(&path);
        h.add("456497901213", "主播杨树", "全新3D僵尸毁灭工程");
        app.history = h;
        app.hist_sel = Some(0);
        app.hist_nav = true;

        let raw = render(&app, 100, 20);
        let s = compact(&raw);
        assert!(s.contains("j/k移动"), "缺 j/k 提示:\n{raw}");
        assert!(s.contains("V全选"), "缺 V 全选提示:\n{raw}");
        assert!(s.contains("d删除"), "缺 d 删除提示:\n{raw}");
        assert!(s.contains("历史导航"), "进入导航后标题要变:\n{raw}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn input_history_lists_more_than_five_entries() {
        let mut app = App::new(false, None);
        let path = std::env::temp_dir().join(format!("dy-ui-hist-many-{}.tsv", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut h = crate::history::History::load_from(&path);
        for i in 0..8 {
            h.add(
                &format!("4564979012{}", 10 + i),
                &format!("主播{i}"),
                &format!("标题{i}"),
            );
        }
        app.history = h;
        app.hist_sel = None;

        let raw = render(&app, 100, 30);
        let s = compact(&raw);
        assert!(s.contains("主播7"), "第 8 条历史也要能看到:\n{raw}");
        assert!(s.contains("主播0"), "最旧一条也要在可视范围里:\n{raw}");
        let _ = std::fs::remove_file(&path);
    }

    // ---- 找直播（分区发现页）----

    fn discover_app() -> App {
        let mut app = App::new(false, None);
        app.screen = Screen::Discover;
        app.discover.loaded_key = Some("4_103".into());
        app.discover.rooms = vec![
            crate::discover::RoomCard {
                rid: "40132179559".into(),
                nick: "玫玫小张帝".into(),
                title: "到美之声文化传媒大舞台了".into(),
                online: "1万+".into(),
                viewers: 10_000,
            },
            crate::discover::RoomCard {
                rid: "544590831076".into(),
                nick: "乱斗教授".into(),
                title: "上分教学".into(),
                online: "368".into(),
                viewers: 368,
            },
        ];
        app
    }

    #[test]
    fn discover_screen_lists_rooms() {
        let app = discover_app();
        let raw = render(&app, 90, 20);
        let s = compact(&raw);
        assert!(s.contains("找直播"), "缺标题:\n{raw}");
        assert!(s.contains("游戏"), "缺分区栏:\n{raw}");
        assert!(s.contains("在线"), "缺表头:\n{raw}");
        assert!(s.contains("玫玫小张帝"), "缺主播:\n{raw}");
        assert!(s.contains("1万+"), "缺在线人数:\n{raw}");
        assert!(s.contains("r下一批"), "缺键位提示:\n{raw}");
    }

    /// 选中的分区和房间即使终端不渲染颜色，也要能看出是哪一行
    #[test]
    fn discover_selection_visible_without_color() {
        let app = discover_app();
        let s = compact(&render(&app, 90, 20));
        assert!(s.contains("▸聊天"), "选中的分区缺少标记:\n{s}");
        assert!(s.contains("▶玫玫小张帝"), "选中的房间缺少标记:\n{s}");
    }

    /// 选中项必须是红底反白，而不是只有 bold
    #[test]
    fn discover_selection_has_red_background() {
        let app = discover_app();
        let buf = buffer(&app, 90, 20);
        let area = buf.area();
        let mut tab_bg = None;
        let mut row_bg = None;
        for y in 0..area.height {
            for x in 0..area.width {
                let cell = &buf[(x, y)];
                let sym = cell.symbol();
                if sym == "▸" {
                    tab_bg = Some(cell.bg);
                }
                if sym == "▶" {
                    row_bg = Some(cell.bg);
                }
            }
        }
        assert_eq!(tab_bg, Some(Color::Red), "分区选中项应为红底");
        assert_eq!(row_bg, Some(Color::Red), "房间选中行应为红底");
    }

    #[test]
    fn discover_screen_shows_loading_and_error() {
        let mut app = discover_app();
        app.discover.rooms.clear();
        app.discover.loading = true;
        let raw = render(&app, 90, 20);
        assert!(compact(&raw).contains("加载"), "loading 提示缺失:\n{raw}");

        app.discover.loading = false;
        app.discover.err = Some("被风控了".into());
        let raw = render(&app, 90, 20);
        assert!(compact(&raw).contains("被风控了"), "错误没显示:\n{raw}");
    }
}
