# Angular Capability Lab

一个使用框架真实 API 的 Angular 21 示例。注解声明在 `src/tools.ts`，实例由 Angular DI 创建；`src/main.ts` 根据运行环境选择独立 Browser Adapter 或 Tauri Transport。

## 运行

使用 Yarn 4，直接在 `examples/tauri-app` 执行：

```sh
yarn install
yarn tauri dev
```

只运行浏览器页面时执行 `yarn dev`，地址：<http://127.0.0.1:1420>。`yarn build` 构建生产资源。启动和构建都会先构建插件根目录的 SDK，首次运行无需手动生成 `dist-js`。

使用 npm 时，从插件根目录执行：

```sh
npm install
npm --prefix examples/tauri-app run tauri dev
```

每次安装选择一种包管理器；npm 安装可能重写 Yarn 锁文件，切回 Yarn 时先重新执行 `yarn install`。根目录 `.yarnrc.yml` 使用 `node-modules`，`resolutions` 将本地 SDK 改为 `portal` 直接链接，避免 `file:../..` 复制整个插件及 Rust 构建缓存。

Angular 运行时 / compiler-cli 为 21.2.25；CLI / build 为已过依赖冷却期的 21.2.24。Piscina 通过两种包管理器的覆盖规则固定为已修复的 5.3.2。冷却期与依赖脚本开关以当前 `.yarnrc.yml` 及用户配置为准。

默认桌面启动会自动启动 Angular 服务；如果已运行 `yarn dev`，请先停止它，或者在示例目录复用该服务：

```sh
yarn tauri dev --config '{"build":{"beforeDevCommand":""}}'
```

## 体验路径

| 操作 | 预期结果 |
| --- | --- |
| 选择 `text.echo`，运行默认输入 | Angular TextService 返回原文本和 Unicode 字符数 |
| 选择 `math.add`，输入 `{"a":21,"b":21}` | 返回 `{"sum":42}` |
| 点击「试试非法参数」后运行 | `INVALID_ARGUMENT`，业务方法不执行 |
| 选择 `system.wait`，运行默认输入 | 等待 500 ms 后返回 |
| 点击「试试超时」后运行 | 等待约 1500 ms，返回 `TIMEOUT` |
| 等待过程中点击「取消调用」 | 返回 `CANCELLED`，清理示例计时器 |
| 选择 `system.fail` 后运行 | 展示 `DEMO_ERROR` |
| 切换「注解声明」 | 显示从真实注册元数据生成的注解 |

桌面页面按钮经 IPC 进入 Rust，与外部 MCP 共用校验、授权、并发、超时和取消管线，记录分别显示 `应用 → Rust → JS` 与 `MCP → Rust → JS`。Rust 在派发前拒绝的参数或权限也会出现在调用记录中。纯浏览器使用独立 Adapter；结果面板显示页面主动发起的调用结果。

示例宿主设置 `backgroundThrottling: "disabled"`，让支持该选项的平台在 WebView 隐藏时继续调度 JS 任务。否则后台挂起可能让异步工具等待超时。该选项支持 macOS 14+ / iOS 17+，Windows / Linux / Android 不支持；详见 [Tauri 配置说明](https://v2.tauri.app/reference/config/#backgroundthrottling)。后台执行的跨平台行为尚未验收。

## MCP 端到端检查

启动桌面应用后，控制台输出 `mcp-connection.json` 的本机路径。该文件包含连接地址及 Authorization header，不能提交或公开。

```sh
node ../../scripts/verify-mcp.mjs "/实际路径/mcp-connection.json"
```

脚本使用 MCP `2025-11-25` 完成初始化、发现和真实 Angular 方法调用。默认端口 38473；可用 `AI_MCP_PORT=0` 自动分配端口，实际地址以连接文件为准。未配置 `AI_MCP_TOKEN` 时，每次启动重新生成凭据。

## 固定本机 MCP 令牌

`yarn tauri` / `npm run tauri` 会自动读取示例目录的 `.env.local`，再调用官方 Tauri CLI。文件格式：

```dotenv
AI_MCP_TOKEN=你的固定令牌
AI_MCP_PORT=38473
```

令牌至少 32 位，仅允许 ASCII 字母、数字、连字符或下划线。新检出时可复制 `.env.local.example` 为 `.env.local`，用 `openssl rand -hex 32` 生成一次令牌并填入；后续无需重新生成。

保存后直接运行：

```sh
yarn tauri dev
```

每次启动会使用同一令牌和端口。`.env.local` 已被 Git 忽略，仅保存在本机；Codex 的 `Authorization` 使用相同的 `Bearer` 令牌。宿主仍会更新 `mcp-connection.json`，该文件中的连接凭据保持相同。

终端中已设置的 `AI_MCP_TOKEN` / `AI_MCP_PORT` 优先于 `.env.local`，遵循 [Node.js 环境文件规则](https://github.com/nodejs/node/blob/v22.19.0/doc/api/cli.md#--env-fileconfig)。直接使用 `cargo run` 时，需要自行提供环境变量。

## 连接 Codex

Codex 可通过 Streamable HTTP 连接这个桌面示例，并在 `~/.codex/config.toml` 配置 URL 与静态 HTTP header。该配置也供桌面客户端使用，详见 [OpenAI 官方 MCP 文档](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)。

1. 在示例目录执行 `yarn tauri dev`，保持桌面应用运行；只运行 `yarn dev` 不会提供 MCP 服务。
2. 在本机打开启动日志给出的 `mcp-connection.json`。macOS 默认路径是 `~/Library/Application Support/com.tauri.ai.framework.example/mcp-connection.json`。
3. 在 Codex 用户配置中添加以下段落。`url` 使用连接文件的 `url`，`Authorization` 使用连接文件中 `headers.Authorization` 的完整值，包含 `Bearer ` 前缀。不要覆盖已有其他配置。

```toml
[mcp_servers.tauri_ai]
url = "http://127.0.0.1:38473/mcp"
http_headers = { Authorization = "Bearer 替换为连接文件中的令牌" }
```

4. 保存后，在客户端的 MCP 设置中重启服务，或重启 Codex 并打开新会话。
5. 发出请求：`请使用 tauri_ai 的 math.add 工具计算 21 + 21，并展示工具返回的 JSON。` 预期结果为 `{"sum":42}`。示例调用记录会显示 `MCP → Rust → JS`。

按上文在 `.env.local` 固定令牌后，Codex header 只需配置一次。没有配置 `AI_MCP_TOKEN` 时，示例会在每次启动生成新令牌，重启应用后需要同步更新 Codex header。

使用 Codex CLI 时也可配置 `bearer_token_env_var = "AI_MCP_TOKEN"`；此时运行 Codex 的进程必须能读取该变量。仅在 Tauri 启动终端导出变量，不会让已经运行的桌面客户端自动获得它。

连接失败时先运行上面的 MCP 检查脚本：401 通常表示 header 缺失或令牌已更新；连接拒绝通常表示桌面示例未运行或 URL / 端口不一致。如果能连接但工具列表为空，先等待 Angular 页面显示「Runtime 已就绪」，再重启客户端的 MCP 服务。

## 新增一个工具

1. 为普通类添加 `@Injectable()`、`@Module('业务模块')`。
2. 为实例方法添加 `@Tool` 和输入输出 JSON Schema，行为提示放 `annotations`，权限、超时和并发放 `policy`。
3. 在 Angular `providers` 中提供实例或工厂，再把类加入 `modules` 列表。适配层只获取已有 Provider；示例的 TextTools 使用 `useFactory`。
4. 重启或刷新应用，工具即随完整注册表发布给 MCP。

实验室页面的示例输入在 `src/app.ts` 的 `INPUTS` 中维护。新增业务工具不需要添加 Rust 命令。

## 当前边界

浏览器模式不提供 MCP HTTP 服务。桌面示例只启用本机 MCP，未集成大模型聊天。示例的 CSP 因运行时 Ajv Schema 编译而允许 `unsafe-eval`；严格 CSP 的构建期预编译路径属于后续工作。
