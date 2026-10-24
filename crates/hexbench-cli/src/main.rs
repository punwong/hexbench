//! `hexbench` — layout inspector and headless runner.

use std::path::PathBuf;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::{Duration, Instant};

use hexbench_core::config;
use hexbench_core::geometry::{bands, exposed_segments};
use hexbench_core::model::{BandPosition, Monitor, Rect, Side, TriggerConfig};
use hexbench_core::{regions, sort_monitors, Region, SliceProbe};

#[derive(Default)]
struct Args {
    demo: bool,
    run: bool,
    dry_run: bool,
    top: bool,
    no_left: bool,
    no_right: bool,
    ratio: Option<f64>,
    seconds: Option<u64>,
    config: Option<PathBuf>,
}

fn main() -> ExitCode {
    // Diagnostic helper: exercise the same SendInput path the tray app uses,
    // without the WebView in the foreground.
    if std::env::args().any(|arg| arg == "--new-desktop") {
        let before = hexbench_core::sys::virtual_desktop_count();
        let sent = hexbench_core::sys::create_virtual_desktop();
        sleep(Duration::from_millis(1200));
        let after = hexbench_core::sys::virtual_desktop_count();
        println!("SendInput 返回 {sent}，桌面数 {before:?} → {after:?}");
        return ExitCode::SUCCESS;
    }

    // WebDAV round trip, for checking a server without opening the UI.
    if let Some(index) = std::env::args().position(|arg| arg == "--sync-test") {
        let rest: Vec<String> = std::env::args().skip(index + 1).collect();
        if rest.len() < 3 {
            eprintln!("用法：hexbench --sync-test <url> <user> <pass>");
            return ExitCode::FAILURE;
        }
        let client = hexbench_core::sync::WebDav::new(&rest[0], &rest[1], &rest[2]);
        match client.test() {
            Ok(()) => println!("PROPFIND ok"),
            Err(error) => {
                eprintln!("PROPFIND 失败：{error}");
                return ExitCode::FAILURE;
            }
        }
        let body = "{\"probe\":true}";
        match client.put("hexbench-probe.json", body) {
            Ok(()) => println!("PUT ok"),
            Err(error) => {
                eprintln!("PUT 失败：{error}");
                return ExitCode::FAILURE;
            }
        }
        match client.get("hexbench-probe.json") {
            Ok(Some(text)) if text == body => println!("GET ok"),
            Ok(Some(text)) => {
                eprintln!("GET 内容不对：{text}");
                return ExitCode::FAILURE;
            }
            Ok(None) => {
                eprintln!("GET 404");
                return ExitCode::FAILURE;
            }
            Err(error) => {
                eprintln!("GET 失败：{error}");
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }

    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => {
            print_help();
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("{message}");
            eprintln!("试试 hexbench --help");
            return ExitCode::from(2);
        }
    };

    if args.run {
        return run_engine(args);
    }

    let mut cfg = TriggerConfig::default();
    if let Some(ratio) = args.ratio {
        cfg.ratio = ratio;
    }
    if args.top {
        cfg.position = BandPosition::Top;
    }
    cfg.enable_left = !args.no_left;
    cfg.enable_right = !args.no_right;

    let mut monitors = if args.demo {
        demo_layout()
    } else {
        hexbench_core::sys::enumerate_monitors()
    };

    if monitors.is_empty() {
        eprintln!("没有检测到显示器");
        return ExitCode::FAILURE;
    }
    sort_monitors(&mut monitors);

    let trigger_regions = regions(&monitors, &cfg);
    print_report(&monitors, &trigger_regions, &cfg, args.demo);

    if !args.demo {
        print_cursor(&monitors, &trigger_regions);
    }

    ExitCode::SUCCESS
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut parsed = Args::default();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "run" => parsed.run = true,
            "--demo" => parsed.demo = true,
            "--dry-run" => parsed.dry_run = true,
            "--layout" => {}
            "--top" => parsed.top = true,
            "--no-left" => parsed.no_left = true,
            "--no-right" => parsed.no_right = true,
            "--ratio" => {
                let value = args.next().ok_or("--ratio 需要一个值，例如 0.1 或 10%")?;
                parsed.ratio = Some(parse_ratio(&value));
            }
            "--seconds" => {
                let value = args.next().ok_or("--seconds 需要一个整数")?;
                parsed.seconds = Some(value.parse().map_err(|_| "秒数不合法")?);
            }
            "--config" => {
                let value = args.next().ok_or("--config 需要一个路径")?;
                parsed.config = Some(PathBuf::from(value));
            }
            "-h" | "--help" => return Ok(None),
            other => return Err(format!("未知参数：{other}")),
        }
    }
    Ok(Some(parsed))
}

fn parse_ratio(raw: &str) -> f64 {
    let raw = raw.trim();
    let value = if let Some(percent) = raw.strip_suffix('%') {
        percent.parse::<f64>().unwrap_or(10.0) / 100.0
    } else {
        raw.parse::<f64>().unwrap_or(0.10)
    };
    if value > 1.0 { value / 100.0 } else { value }
}

fn demo_layout() -> Vec<Monitor> {
    vec![
        Monitor {
            rect: Rect::new(0, 0, 3440, 1440),
            work: Rect::new(0, 0, 3440, 1392),
            primary: true,
            device: r"\\.\DISPLAY1".to_string(),
        },
        Monitor {
            rect: Rect::new(-1080, -232, 0, 1688),
            work: Rect::new(-1080, -232, 0, 1688),
            primary: false,
            device: r"\\.\DISPLAY2".to_string(),
        },
    ]
}

fn print_report(monitors: &[Monitor], trigger_regions: &[Region], cfg: &TriggerConfig, demo: bool) {
    println!(
        "HexBench {} · {}",
        env!("CARGO_PKG_VERSION"),
        if demo { "示例布局（方案第三节）" } else { "本机实时布局" }
    );
    println!();

    for (index, monitor) in monitors.iter().enumerate() {
        let primary = if monitor.primary { "主屏" } else { "" };
        println!(
            "  {} {}×{} {}  {}  L {}  T {}  R {}  B {}",
            pad(&format!("屏 {index}"), 6),
            monitor.rect.width(),
            monitor.rect.height(),
            pad(primary, 6),
            monitor.device,
            monitor.rect.left,
            monitor.rect.top,
            monitor.rect.right,
            monitor.rect.bottom,
        );
    }

    let band_label = match cfg.position {
        BandPosition::Bottom => "底部",
        BandPosition::Top => "顶部",
        BandPosition::Both => "上下（四角）",
    };
    println!();
    println!("  触发带：{} {:.0}%（按每块屏自己的高度）", band_label, cfg.ratio * 100.0);
    println!();

    let probe = SliceProbe(monitors);
    let mut total_exposed = 0;

    for (index, monitor) in monitors.iter().enumerate() {
        let bands = bands(monitor, cfg);
        for side in Side::all() {
            if !cfg.side_enabled(side) {
                continue;
            }
            let segments = exposed_segments(&probe, monitors, index, side);
            let exposed: i32 = segments.iter().copied().map(|(a, b)| b - a).sum();
            let in_band: i32 = segments
                .iter()
                .copied()
                .map(|(a, b)| {
                    bands
                        .iter()
                        .map(|(band_y0, band_y1)| (b.min(*band_y1) - a.max(*band_y0)).max(0))
                        .sum::<i32>()
                })
                .sum();
            total_exposed += in_band;

            let status = if in_band > 0 { "✓ 可触发" } else { "✕ 接缝" };
            let percent = if monitor.rect.height() > 0 {
                f64::from(in_band) / f64::from(monitor.rect.height()) * 100.0
            } else {
                0.0
            };
            println!(
                "  {} x={:<6} {} 外露 {:>5} px  带内 {:>4} px  {:>3.0}%",
                pad(&format!("屏 {index} {}边缘", side.label()), 16),
                side.edge_x(monitor),
                pad(status, 10),
                exposed,
                in_band,
                percent,
            );
        }
    }

    println!();
    println!("  合计 {} 段可触发区，带内总长 {} px", trigger_regions.len(), total_exposed);
    if cfg.position == BandPosition::Bottom && cfg.ratio >= 0.10 {
        println!("  提示：底部触发带可能覆盖任务栏，v0.1 会按类名排除（见方案 2.2）");
    }
}

fn print_cursor(monitors: &[Monitor], trigger_regions: &[Region]) {
    println!();
    let Some((x, y)) = hexbench_core::sys::cursor_pos() else {
        println!("  光标：读取失败");
        return;
    };
    match monitors.iter().position(|m| m.rect.contains(x, y)) {
        Some(index) => {
            let hit = trigger_regions.iter().find(|r| r.edge_x == x && y >= r.y0 && y < r.y1);
            match hit {
                Some(region) => println!(
                    "  光标 ({x}, {y}) → 屏 {index}，命中{}边缘 y {}..{}",
                    region.side.label(),
                    region.y0,
                    region.y1
                ),
                None => println!("  光标 ({x}, {y}) → 屏 {index}，未命中触发区"),
            }
        }
        None => println!("  光标 ({x}, {y}) → 不在任何显示器上"),
    }
}

fn run_engine(args: Args) -> ExitCode {
    let path = args
        .config
        .clone()
        .or_else(config::default_path)
        .unwrap_or_else(|| PathBuf::from("config.json"));
    let (cfg, source) = config::load(&path);

    println!(
        "HexBench {} · 运行{}",
        env!("CARGO_PKG_VERSION"),
        if args.dry_run { "（dry-run，不会真的切换）" } else { "" }
    );
    println!(
        "  配置：{}（{}）",
        path.display(),
        match source {
            config::LoadSource::Main => "已加载",
            config::LoadSource::Backup => "从 .bak 恢复",
            config::LoadSource::Default => "默认值",
        }
    );
    println!(
        "  触发：{} {:.0}%，停留 {} ms，冷却 {} ms，轮询 {} ms",
        match cfg.trigger.position {
            BandPosition::Bottom => "底部",
            BandPosition::Top => "顶部",
            BandPosition::Both => "上下（四角）",
        },
        cfg.trigger.ratio * 100.0,
        cfg.trigger.dwell_ms,
        cfg.trigger.cooldown_ms,
        cfg.trigger.poll_ms,
    );

    let mut engine = hexbench_core::engine::Engine::new(cfg.clone(), args.dry_run);
    println!(
        "  显示器 {} 块，触发区 {} 段；把鼠标推到边缘的角上试试（Ctrl+C 退出）",
        engine.monitors().len(),
        engine.regions().len()
    );
    for region in engine.regions() {
        println!(
            "    屏 {} {}边缘 x={}，y {}..{}",
            region.monitor,
            region.side.label(),
            region.edge_x,
            region.y0,
            region.y1
        );
    }
    println!();

    let poll = Duration::from_millis(cfg.trigger.poll_ms.max(10));
    let deadline = args.seconds.map(|s| Instant::now() + Duration::from_secs(s));
    let mut last_guard: Option<&'static str> = None;
    let mut last_hit = None;

    loop {
        let tick = engine.tick();

        if tick.hit != last_hit {
            match tick.hit {
                Some(hit) => println!(
                    "[{:>7} ms] 进入触发区 · 屏 {} {}边缘（停住 {} ms 生效）",
                    tick.at_ms,
                    hit.monitor,
                    hit.side.label(),
                    cfg.trigger.dwell_ms
                ),
                None => println!("[{:>7} ms] 离开触发区", tick.at_ms),
            }
            last_hit = tick.hit;
        }

        if let Some(fire) = tick.fire {
            let direction = match fire.hit.side {
                Side::Left => "上一个",
                Side::Right => "下一个",
            };
            println!(
                "[{:>7} ms] 触发 · 屏 {} {}边缘 → {}桌面{}",
                tick.at_ms,
                fire.hit.monitor,
                fire.hit.side.label(),
                direction,
                if args.dry_run { "（dry-run）" } else if tick.switched { "" } else { "（发送失败）" }
            );
        }

        if tick.guard != last_guard {
            if let Some(reason) = tick.guard {
                println!("[{:>7} ms] 跳过：{}", tick.at_ms, guard_label(reason));
            }
            last_guard = tick.guard;
        }

        if let Some(deadline) = deadline {
            if Instant::now() >= deadline {
                break;
            }
        }
        sleep(poll);
    }

    let stats = engine.stats();
    println!();
    println!(
        "退出：轮询 {} 次，触发 {} 次，发送失败 {} 次",
        stats.ticks, stats.fires, stats.send_failures
    );
    ExitCode::SUCCESS
}

fn guard_label(reason: &str) -> &'static str {
    match reason {
        "disabled" => "总开关关闭",
        "locked" => "锁屏 / 安全桌面",
        "button" => "鼠标按键按下",
        "fullscreen" => "前台全屏",
        "taskbar" => "光标在任务栏上",
        _ => "已跳过",
    }
}

fn print_help() {
    println!(
        "hexbench — HexBench 布局检查与无界面引擎\n\n\
         用法:\n\
         \x20 hexbench [--layout] [选项]      打印显示器与可触发区\n\
         \x20 hexbench run [选项]             前台运行引擎（v0.1 无界面）\n\n\
         通用选项:\n\
         \x20 --demo          用方案第三节的示例布局\n\
         \x20 -h, --help      显示帮助\n\n\
         布局选项:\n\
         \x20 --ratio <n>     触发带比例，0.1 或 10%（默认 0.10）\n\
         \x20 --top           触发带贴顶部\n\
         \x20 --no-left / --no-right\n\
         \x20                 关闭左 / 右边缘\n\n\
         运行选项:\n\
         \x20 --dry-run       只打印命中，不发送 Win+Ctrl+方向键\n\
         \x20 --seconds <n>   跑 n 秒后自动退出（方便脚本验证）\n\
         \x20 --config <path> 指定配置文件"
    );
}

/// Display width of a string, counting CJK as two columns.
fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| {
            let u = c as u32;
            let wide = (0x1100..=0x115F).contains(&u)
                || (0x2E80..=0xA4CF).contains(&u)
                || (0xAC00..=0xD7A3).contains(&u)
                || (0xF900..=0xFAFF).contains(&u)
                || (0xFE30..=0xFE6F).contains(&u)
                || (0xFF00..=0xFF60).contains(&u)
                || (0xFFE0..=0xFFE6).contains(&u);
            if wide { 2 } else { 1 }
        })
        .sum()
}

fn pad(text: &str, width: usize) -> String {
    let mut out = text.to_string();
    for _ in display_width(text)..width {
        out.push(' ');
    }
    out
}
