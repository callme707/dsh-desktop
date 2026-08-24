# dsh desktop 设计验收

## 对照目标

- primary source visual truth path: `C:\Users\lijialin\.codex\generated_images\01a03246-91fc-7fc1-9a42-ed692f1f1705\exec-2fe2caa5-bb8d-4c60-b02e-9b3a2639689f.png`
- source pixels / density: `1807 × 870 @ 120 DPI`
- implementation screenshot: `C:\Users\lijialin\.codex\visualizations\2026\08\24\01a03246-91fc-7fc1-9a42-ed692f1f1705\dsh-titlebar-qa\native-window-final.png`
- implementation pixels / density: `1618 × 959 @ 120 DPI`；Tauri 内容区为 `1280 × 760 CSS px`，Windows `deviceScaleFactor = 1.25`，截图包含系统阴影边界
- combined comparison input: `C:\Users\lijialin\.codex\visualizations\2026\08\24\01a03246-91fc-7fc1-9a42-ed692f1f1705\dsh-titlebar-qa\comparison.html`
- state: 深色主题、dsh 内核就绪、普通窗口、真实远程 dsh 内容页
- supplementary update-flow source: `C:\Users\lijialin\.codex\generated_images\01a03246-91fc-7fc1-9a42-ed692f1f1705\exec-8c473be5-6b11-47e2-8655-21636efeec72.png`
- supplementary update-flow implementation: `C:\Users\lijialin\.codex\visualizations\2026\08\24\01a03246-91fc-7fc1-9a42-ed692f1f1705\dsh-desktop-design-qa\update-1280x820.png`

## Findings

- 最终对照未发现可执行的 P0、P1 或 P2 问题。
- 字体与排版：顶栏使用 `Segoe UI Variable / Segoe UI / Microsoft YaHei UI` 系统栈；品牌、内核状态、面包屑和按钮的字号、字重、行高及截断层级与选择稿一致。dsh 正文由远程产品自身渲染，不由桌面壳覆写。
- 间距与布局：顶栏和内容严格分层；品牌区分界跟随真实 dsh 侧栏的 `280 CSS px`，在最终 `125% DPI` 截图中两条分界重合。选择稿中的较宽侧栏比例来自稿件固定画布，实现以第三方 dsh 当前响应式宽度为产品约束，保留“上下分栏对齐”的设计规则而不硬编码稿件像素。
- 颜色与视觉 token：石墨黑主背景、略亮侧栏、低对比分隔线、蓝色强调和绿色就绪态均匹配选择稿；按钮 hover、focus、禁用和失焦态使用同一 token 体系。
- 图片与图标：品牌图标使用独立 PNG；刷新、最小化、最大化/还原和关闭使用 Tabler Icons 的 MIT 授权 SVG 文件。WebView2 中所有图标均可见，没有内联 SVG、CSS 绘图、emoji 或占位资产。
- 文案与内容：`dsh desktop`、`内核已就绪`、`工作台 / DeepSeek Harness` 与选择稿一致；更新页文案与实际 npm 行为一致。会话标题与正文属于动态 dsh 内容，不作为静态稿件文案差异。
- 可访问性：四个顶栏按钮均有中文可访问名称、title、focus-visible 状态；Windows UI Automation 可识别 `检查 dsh 更新 / 最小化 / 最大化或还原 / 关闭`。状态文本使用 `role=status` 与 `aria-live=polite`，并支持 reduced motion。
- 响应式：静态顶栏在 `1024 CSS px` 下品牌区为 `280px`；`900 CSS px` 下进入紧凑态，品牌区为 `56px`、文字状态收起、所有操作按钮仍完整可见，页面横向溢出为 0。

## Full-view comparison evidence

- 参考稿与最终原生截图已在 `comparison.html` 中置于同一浏览器比较输入，按各自完整宽度等比例显示；两张图片均为 `120 DPI`。
- 重点核对了整体层级、深色面结构、品牌/状态/面包屑/窗口操作的顺序，以及顶栏分界与 dsh 侧栏的连续关系。
- 实现保留了真实产品约束：远程 dsh 的会话内容与侧栏响应式宽度可变化；桌面壳只控制持久顶栏和两个本地状态页，不向 dsh 注入样式。

## Focused region comparison evidence

- `comparison.html` 的第二组对照以相同展示宽度裁出两张图的顶部区域，专门检查品牌、内核状态、面包屑、刷新入口、分隔线和三个窗口按钮。
- 运行时聚焦截图：`C:\Users\lijialin\.codex\visualizations\2026\08\24\01a03246-91fc-7fc1-9a42-ed692f1f1705\dsh-titlebar-qa\native-window-final.png`
- 更新链路聚焦对照：`C:\Users\lijialin\.codex\visualizations\2026\08\24\01a03246-91fc-7fc1-9a42-ed692f1f1705\dsh-desktop-design-qa\reference-workflow.png` 与 `implementation-workflow.png`

## Comparison history

1. `[P1]` 原实现使用 Windows 白色原生标题栏与菜单，和 dsh 深色界面割裂。已改为一个无边框 Tauri 窗口承载隔离的 `chrome` 与 `content` WebView，并把更新入口移入持久顶栏；原生截图确认白条和菜单已消失。
2. `[P2]` 首轮真实窗口里顶栏品牌区为 `355 CSS px`，而当前 dsh 侧栏为 `280 CSS px`，分界错开约 `75 CSS px`。已把 `--sidebar-width` 改为 `280px`；最终截图中上下分界重合。
3. `[P2]` 外链 SVG 使用 `currentColor` 并叠加 CSS filter，在真实 WebView2 中窗口按钮图标不可见。已为 Tabler 资产写入显式 `#c7cad1` stroke，并移除常态 filter；最终截图和 UI Automation 均确认四个按钮存在且可见。
4. `[P2]` 原默认 `1280 × 872 CSS px` 在 1080p / 125% DPI 下约为 `1600 × 1090` 物理像素，底部会被任务栏裁切。已改为 `1280 × 760`；最终窗口为 `1618 × 959`（含系统边界），完整位于工作区。
5. 更新页早期 `[P2]` 主分隔线偏右、阶段节距偏密，已调整为 `1.13fr / 1px / 1fr` 与 128px 阶段节距；`1280 × 820` 复测对齐。
6. 更新页早期 `[P2]` 标题比例偏小，已调整大标题与品牌字，并重新平衡说明间距；复测无截断。
7. 更新页早期 `900 × 600` 高度为 652px、底部元数据被裁切；增加紧凑高度断点后，页面与视口均为 `900 × 600`，底部完整可见。

## Runtime verification

- `cargo fmt --all -- --check`：通过。
- `cargo test --offline`：4 passed，0 failed。
- `cargo clippy --offline --all-targets -- -D warnings`：通过，无警告。
- 真实 Tauri / WebView2：自定义拖动有效；最大化 → 还原 → 最大化 → 还原状态均正确；最小化后可恢复；关闭按钮会终止应用进程。
- 手动更新入口：真实点击后读取本地与 npm registry，二者均为 `v0.1.1-rc.2`；只显示“当前已是最新版本”，未执行 `npm install`。
- 应用内浏览器：`chrome.html` 在 1024 与 900 CSS px 下完成 DOM、资产、溢出与响应式检查；warning/error 均为 0。
- 安全边界：capability 仅匹配 `webviews: ["chrome"]`；远程 dsh 所在 `content` WebView 无 Tauri IPC。

## Open Questions

- 无阻塞问题。

## Implementation Checklist

- [x] 选择稿 2 的持久深色顶栏
- [x] 顶栏与真实 dsh 侧栏分界对齐
- [x] 更新入口及状态反馈整合到顶栏
- [x] WebView2 图标可见性
- [x] 拖动、最小化、最大化/还原、关闭
- [x] 1024 / 900 CSS px 响应式与无溢出
- [x] 125% DPI 默认窗口完整可见
- [x] 更新页与启动页视觉系统
- [x] 字体、间距、颜色、资产、文案、可访问性和控制台检查

## Follow-up Polish

- `[P3]` 若以后确定正式品牌字体，可随安装包内置字体文件，进一步消除不同 Windows 版本的字重差异。
- `[P3]` 选择稿的轻微环境光保持未实现；当前纯色表面更贴合 dsh 本体，也避免低质量渐变或装饰资产。

final result: passed
