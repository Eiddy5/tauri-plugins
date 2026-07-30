# Screenshot Native Demo

这个示例用于验证 `tauri-plugin-screenshot` 的完整调用链：多屏原生浮层、光标像素放大镜、窗口悬停预选、自由框选、二次确认，以及读取 PNG 字节并在 WebView 中预览。

## 运行要求

- Tauri 2 所需的系统依赖
- Rust `1.77.2` 或更高版本
- Node.js 和任意一个 JavaScript 包管理器
- macOS 15.2+ 或 Windows 10/11

## 启动

选择当前环境已经安装的一个包管理器：

```sh
# pnpm
pnpm install
pnpm tauri dev

# npm
npm install
npm run tauri dev

# Yarn
yarn install
yarn tauri dev

# Bun
bun install
bun run tauri dev
```

不要混用不同包管理器生成的 `node_modules`。Tauri 的前置命令直接调用项目内安装的 Vite，不会在 `beforeDevCommand` 中依赖某个全局包管理器。

## 验证内容

1. 点击“开始截图”后，原生浮层和十字像素放大镜应立即出现。
2. 在任意显示器中悬停窗口时，应自动预选最上层窗口。
3. 单击窗口或拖动创建选区后，应进入二次确认。
4. 确认后，页面应显示 PNG 预览及物理像素区域信息。
5. `Esc`、鼠标右键或工具条取消按钮应返回取消状态。

macOS 首次截图需要允许系统“屏幕录制”权限；授权后如果系统提示，请重启示例应用。

插件安装、权限和 API 说明见仓库根目录的 [README](../../README.md)。
