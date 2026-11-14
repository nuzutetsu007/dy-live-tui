# dy-live-tui

[![CI](https://github.com/nuzutetsu007/dy-live-tui/actions/workflows/ci.yml/badge.svg)](https://github.com/nuzutetsu007/dy-live-tui/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/nuzutetsu007/dy-live-tui?display_name=tag)](https://github.com/nuzutetsu007/dy-live-tui/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

抖音直播间 → 带签名的直链 → 选清晰度 → 测连通 → 本地 mpv 播放。
纯 Rust（`ureq` + `ratatui`），解析过程不调用 Python / curl / 任何外部脚本；只有最后的播放会 `spawn mpv`。

```
┌ dy-live TUI ────────────────────────────────────────────────┐
│ ▶ 主播杨树 · 全新3D僵尸毁灭工程   (在线 3000+)               │
├─────────────────────────────────────────────────────────────┤
│ 质量        分辨率        协议  连通    延迟     直链        │
│ 原画        1920x1080@60  HLS   ✓ 200   295 ms   _or4.m3u8   │
│ 原画        1920x1080@60  FLV   ✓ 200   325 ms   _or4.flv    │
│ 蓝光        1920x1080@45  HLS   ✓ 200   338 ms   _uhd.m3u8   │
│ 超清        1280x720@30   FLV   ✓ 200   312 ms   _hd.flv     │
│ 标清        960x540@25    HLS   ✓ 200  1377 ms   _ld.m3u8    │
├─────────────────────────────────────────────────────────────┤
│ ↑/↓ 选择  Enter 播放  f 强制播放  p 重测  r 重解析  q 退出   │
└─────────────────────────────────────────────────────────────┘
```

## 依赖

- Rust 1.75+（`cargo build --release`）
- 播放需要 `mpv` 在 `PATH` 里（不装也能用 `--resolve` / `--dry-run`）

## 构建 & 安装

直接下预编译二进制（推荐）：

```bash
# Linux x86_64
curl -L https://github.com/nuzutetsu007/dy-live-tui/releases/latest/download/dy-live-tui-x86_64-unknown-linux-gnu.tar.gz \
  | tar xz
install -m755 dy-live-tui ~/.local/bin/

# 其它平台把文件名换成：
#   dy-live-tui-aarch64-unknown-linux-gnu.tar.gz
#   dy-live-tui-aarch64-apple-darwin.tar.gz
#   dy-live-tui-x86_64-pc-windows-msvc.zip
```

每个包旁边都有对应的 `.sha256`，`SHA256SUMS.txt` 里是全部。

从源码：

```bash
git clone https://github.com/nuzutetsu007/dy-live-tui
cd dy-live-tui
cargo build --release
ln -sf "$PWD/target/release/dy-live-tui" ~/.local/bin/dy-live-tui
```

## 用法

```bash
dy-live-tui                          # 打开输入页
dy-live-tui 456497901213             # 带初始房间号，直接开始解析
dy-live-tui https://live.douyin.com/456497901213   # 直接粘链接也行
dy-live-tui --dry-run 456497901213   # 只解析+探测，不启动 mpv
dy-live-tui --resolve 456497901213   # 非交互：打印房间信息 + 全部直链
```

`--dry-run` 选中后不播放，把直链写入 `~/.cache/dy-live-tui/last-url`，可以自己喂给别的播放器：

```bash
mpv "$(cat ~/.cache/dy-live-tui/last-url)"
```

## 按键

| 页面 | 按键 | 作用 |
| --- | --- | --- |
| 输入 | `Enter` | 解析 |
| 输入 | `↑` `↓` | 在「最近打开」里选择（自动填入输入框） |
| 输入 | `Ctrl-U` | 清空输入 |
| 输入 | `Esc` | 退出 |
| 加载 | `Esc` / `q` | 取消解析 |
| 列表 | `↑` `↓` / `j` `k` | 选择候选 |
| 列表 | `Enter` | 用 mpv 播放 |
| 列表 | `f` | 强制播放（探测失败的也播） |
| 列表 | `p` | 重新探测所有直链 |
| 列表 | `r` | 重新解析房间（拿新 sign） |
| 列表 | `Esc` | 返回输入页 |
| 列表 | `q` | 退出 |

支持括号粘贴（bracketed paste），链接里带 `?enter_from=...` 也能识别。

## 历史记录

只记录**用 mpv 成功打开过**（退出码 0）的直播间，保存在
`~/.cache/dy-live-tui/history.tsv`，最近 12 条，同房间自动去重并挪到最前。

```
最近打开  ↑/↓ 选择，回车重新打开
❯ 456497901213  主播杨树 · 全新3D僵尸毁灭工程
  123456789     某人 · 标题B
```

启动时如果没带参数，会把上次看的房间号直接填进输入框——**按一次回车就能重看**。
`--dry-run` 不会写入历史，解析失败/下播/播放失败也不会。

清空历史：`rm ~/.cache/dy-live-tui/history.tsv`。

## 原理

1. `GET https://live.douyin.com/<rid>`，顺手拿 `ttwid` cookie（可选，拿不到也继续）。
2. `GET https://live.douyin.com/webcast/room/web/enter/?...&web_rid=<rid>`
   → `data.data[0].stream_url` 里的 `hls_pull_url_map` / `flv_pull_url`，**只有这里返回的地址带 `sign`**。
   页面 HTML 内嵌的地址没有签名，直接拉会被 CDN 403，所以不能从页面正则抠地址。
3. 优先读 `stream_url.live_core_sdk_data.pull_data` 里的完整档位表：
   `options.qualities` 给出 sdk_key / 中文名 / 分辨率 / 帧率 / level，
   `stream_data`（一个 JSON 字符串）给出各档的 `hls` / `flv` 地址。
   这里比老的 `hls_pull_url_map` 多出 **原画（`origin` / `_or4`，1080p 60fps）**，
   实测约 15 Mbps；老房间没有这份数据时退回 `hls_pull_url_map`（`FULL_HD1` 即蓝光 45fps）。
4. 按 `level` 从高到低排序（原画 → 蓝光 → 超清 → 高清 → 标清），每档 HLS 优先、FLV 兜底，按 URL 去重。
5. 每条直链发 `Range: bytes=0-2047` 请求探测（只看状态码，不下载内容），4 路并发。
6. 选中后暂停 TUI（退出 alternate screen）→ `mpv --ytdl=no --force-media-title=... <url>` → mpv 退出后恢复 TUI，退出码为 0 才记入历史。

签名 URL 形如：

```
http://pull-hls-e13.douyincdn.com/third/stream-<stream_id>_uhd.m3u8
    ?expire=<unix_ts>&sign=<32位 md5>&arch_hrchy=h1
```

`expire` 一般 12 小时左右；主播重开播会换 stream key，旧链接失效——所以播放前尽量一次选好，过期就按 `r` 重新解析。

## 测试

```bash
cargo test                  # 单元测试：房间号解析、探测状态判定、TestBackend 渲染
cargo test -- --ignored     # 真实网络集成测试（需要房间正在直播）
```

非 TTY 环境下冒烟测试（不会弹窗）：

```bash
cargo build
{ printf '456497901213\r'; sleep 12; printf '\r'; sleep 2; printf 'q'; } \
  | script -qec 'stty rows 24 cols 100; ./target/debug/dy-live-tui --dry-run' /dev/null
cat ~/.cache/dy-live-tui/last-url
```

## 已知限制

- **编码只有 H.264**：`stream_url.extra` 里 `h265_enable` / `bytevc1_enable` 都是 false，
  `options.qualities[].v_codec` 全是 `264`；换 `support_h265=1` / `v_codec=265` 也不会变。
  是否给 H.265 取决于主播推流和平台转码，web 接口这一路基本只发 H.264；AV1 直播拉流目前没有。
- 只支持普通直播间；VR、连麦、付费等模式 web 端接口可能不给 `stream_url`。
- 风控（`status=0` 且无数据）时只能重试，或挂上浏览器里的 cookie。
- 播放交给系统 mpv，若 `~/.config/mpv/mpv.conf` 里有 `fs=yes` 会全屏；mpv 载入失败时不会开窗，看它的 stderr 即可。
- 不做录制，只做「解析 + 交给播放器」。
