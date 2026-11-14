mod app;
mod douyin;
mod ui;

use app::{App, Screen};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, stdout, Stdout};
use std::time::Duration;

type Term = Terminal<CrosstermBackend<Stdout>>;

const HELP: &str = "\
dy-live-tui — 抖音直播间 → 直链 → mpv

用法:
  dy-live-tui [房间号或链接]          进入 TUI（可带初始房间号）
  dy-live-tui --resolve <房间号>      不启动 TUI，直接打印房间信息与直链
  dy-live-tui --dry-run ...           选完后不调 mpv，只把直链写到缓存文件

参数:
  -n, --dry-run          只解析/探测，不启动 mpv（直链写到 ~/.cache/dy-live-tui/last-url）
      --resolve <rid>    非交互模式
  -h, --help             帮助
  -V, --version          版本

TUI 内按键:
  输入页   Enter 解析    Ctrl-U 清空    Esc 退出
  列表页   ↑/↓ 或 j/k 选择   Enter 播放   f 强制播放   p 重测   r 重解析   Esc 返回   q 退出
";

fn init() -> io::Result<Term> {
    terminal::enable_raw_mode()?;
    execute!(
        stdout(),
        EnterAlternateScreen,
        crossterm::event::EnableBracketedPaste,
        crossterm::cursor::Hide
    )?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout()))?;
    term.clear()?;
    Ok(term)
}

fn restore(term: &mut Term) -> io::Result<()> {
    terminal::disable_raw_mode()?;
    execute!(
        term.backend_mut(),
        LeaveAlternateScreen,
        crossterm::event::DisableBracketedPaste,
        crossterm::cursor::Show
    )?;
    term.show_cursor()?;
    Ok(())
}

fn cache_path(name: &str) -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".cache")
        });
    base.join("dy-live-tui").join(name)
}

fn print_resolve(rid: &str) -> io::Result<()> {
    match douyin::resolve(rid) {
        Ok((room, cands)) => {
            println!(
                "房间: {} · {}\n在线: {}   房间号: {}   status={}",
                room.nick, room.title, room.online, room.rid, room.status
            );
            println!();
            for c in &cands {
                let p = douyin::probe(&c.url);
                let mark = if p.ok() {
                    format!("ok  {:>3}", p.code)
                } else {
                    format!("err {:>3}", p.code)
                };
                println!(
                    "{:<7} {:<4} {} {:>5}ms  {}",
                    c.quality,
                    c.proto.as_str(),
                    mark,
                    p.ms,
                    c.url
                );
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("解析失败: {e}");
            std::process::exit(1);
        }
    }
}

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut dry_run = false;
    let mut initial: Option<String> = None;
    let mut resolve_rid: Option<String> = None;

    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("dy-live-tui {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-n" | "--dry-run" => dry_run = true,
            "--resolve" => resolve_rid = args.next(),
            other => initial = Some(other.to_string()),
        }
    }

    if let Some(r) = resolve_rid {
        let rid = douyin::normalize_rid(&r).unwrap_or_else(|| {
            eprintln!("房间号/链接不对: {r}");
            std::process::exit(2);
        });
        return print_resolve(&rid);
    }

    // 终端异常时也要还原屏幕
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
        original_hook(info);
    }));

    let mut term = init()?;
    let mut app = App::new(dry_run, initial);

    let result = (|| -> io::Result<()> {
        loop {
            term.draw(|f| ui::draw(f, &app))?;

            app.poll();

            if event::poll(Duration::from_millis(120))? {
                match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => app.handle_key(k),
                    Event::Paste(s) => app.paste(&s),
                    _ => {}
                }
            }

            if app.screen != Screen::Input || app.is_probing() {
                app.spin();
            }

            // 播放：暂停 TUI → 交给 mpv → 回来
            if let Some((url, title)) = app.pending_play.take() {
                if app.dry_run {
                    let path = cache_path("last-url");
                    if let Some(dir) = path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = std::fs::write(&path, &url);
                    app.msg = format!("dry-run: 直链已写入 {}", path.display());
                } else {
                    restore(&mut term)?;
                    let res = douyin::play_mpv(&url, &title);
                    term = init()?;
                    app.on_play_done(res);
                }
            }

            if app.quit {
                break;
            }
        }
        Ok(())
    })();

    restore(&mut term)?;
    result
}
