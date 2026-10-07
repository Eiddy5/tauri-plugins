# 开源 MCP 框架：命名与功能划分调研

查阅日期：2026-10-06。以下记录来自项目自己的文档、源码及许可证，未安装或运行这些外部框架；官网和主分支 API 可能继续变化。

本项目当前导出 `@Module`、`@Tool`；`@Resource`、`@Prompt` 尚未实现。下文的 Resource/Prompt 用法比较与本项目建议均不能当作已经可调用的 SDK API。见 [当前导出](../guest-js/index.ts)、[装饰器实现](../guest-js/decorators.ts)。

## 1. Resource 所表达的能力

Resource 是客户端通过 URI 读取的内容：文档、数据库记录、报告等都可以成为资源。实际返回 `contents`，其中每项包含 URI，以及文本 `text` 或 Base64 二进制 `blob`，并可提供 `mimeType`。JSON 数据通常作为 `application/json` 文本内容返回。内容可以在读取时动态生成。见 [官方 TypeScript SDK Resources](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/docs/servers/resources.md)。

例如 `docs://guide` 可以标识指南；`docs://document/123` 可以标识某篇文档；`docs://document/{id}` 表达一类可读取文档的 URI 模板。模板能够供客户端发现，其具体实例仍通过资源读取获取；是否枚举实例由注册时的列表回调决定。见 [官方 TypeScript SDK URI 模板](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/docs/servers/resources.md#add-a-resource-template)。

以下是对本项目的划分建议：

| 能力 | 适合的业务入口 | 返回内容 |
| --- | --- | --- |
| Tool | 搜索、计算、查询、创建、修改、触发工作流 | 调用结果，可以包含结构化数据 |
| Resource | 按 URI 获取文档、规则、报告、文件或记录，供客户端作为上下文使用 | 带 URI 和内容类型的资源内容 |
| Prompt | 根据参数组织一组可复用消息 | 消息序列 |

只读查询也可以是 Tool。本项目应按客户端如何发现和使用能力决定分类，不能把所有只读方法自动变成 Resource。这一判断参考官方 SDK 对资源与工具控制方式的划分，以及两者不同的注册和请求接口。见 [Resources](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/docs/servers/resources.md)、[Tools](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/docs/servers/tools.md)。

## 2. 可借鉴的开源项目

| 项目 | 许可证快照 | 定位 |
| --- | --- | --- |
| [PrefectHQ/FastMCP](https://github.com/PrefectHQ/fastmcp) | [Apache-2.0](https://github.com/PrefectHQ/fastmcp/blob/main/LICENSE) | 独立 Python 框架；用 `from fastmcp import FastMCP` 创建服务，围绕 tools/resources/prompts 组织能力。见 [服务概览](https://gofastmcp.com/servers/server)。 |
| [rekog-labs/MCP-Nest](https://github.com/rekog-labs/MCP-Nest) | [MIT](https://github.com/rekog-labs/MCP-Nest/blob/main/LICENSE) | TypeScript/NestJS 框架；使用能力装饰器，并接入 NestJS 的依赖注入及处理管线。见 [项目 README](https://github.com/rekog-labs/MCP-Nest)。 |
| [官方 Python SDK](https://github.com/modelcontextprotocol/python-sdk) | [MIT](https://github.com/modelcontextprotocol/python-sdk/blob/main/LICENSE) | v1 的高层服务器叫 `FastMCP`；当前官网与 main 的 v2 API 改为 `MCPServer`。它与独立 `fastmcp` 包要分开辨认。见 [官方迁移说明](https://py.sdk.modelcontextprotocol.io/migration/#fastmcp-renamed-to-mcpserver)。 |
| [官方 TypeScript SDK](https://github.com/modelcontextprotocol/typescript-sdk) | [MIT 向 Apache-2.0 过渡；文档 CC-BY-4.0](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/LICENSE) | 当前 main 的高层入口为 `McpServer`，采用注册函数。见 [官方 README](https://github.com/modelcontextprotocol/typescript-sdk)。 |

官方 TypeScript SDK 的 LICENSE 保留尚未获得重授权同意的 MIT 贡献；因此不能把整个当前仓库简单写成单一 MIT 或单一 Apache-2.0。见 [LICENSE](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/LICENSE)。

## 3. 实际 API 名称比较

| 项目 / 查阅范围 | Tool | 固定 Resource | Resource 模板 | Prompt |
| --- | --- | --- | --- | --- |
| 独立 FastMCP 当前官网 | `@mcp.tool` | `@mcp.resource("config://app")` | 同一个 `@mcp.resource("docs://{id}")` | `@mcp.prompt` |
| MCP-Nest 当前 main | `@Tool` | `@Resource` | 单独的 `@ResourceTemplate` | `@Prompt` |
| 官方 Python SDK v1.x | `@mcp.tool()` | `@mcp.resource(uri)` | 同一个 `@mcp.resource(uri)` | `@mcp.prompt()` |
| 官方 Python SDK 当前 v2 文档 | 继续使用 `@mcp.tool()`、`@mcp.resource(uri)`、`@mcp.prompt()`；服务器类改为 `MCPServer` | 同左 | 同左 | 同左 |
| 官方 TypeScript SDK 当前 main | `registerTool(...)` | `registerResource(...)` | `registerResource(..., new ResourceTemplate(...), ...)` | `registerPrompt(...)` |

各行依据：

- 独立 FastMCP：[Tools](https://gofastmcp.com/servers/tools)、[Resources & Templates](https://gofastmcp.com/servers/resources)、[Prompts](https://gofastmcp.com/servers/prompts)。固定 URI 与模板确实共享 resource 装饰器。
- MCP-Nest：[装饰器导出](https://github.com/rekog-labs/MCP-Nest/blob/main/packages/mcp-nest/src/mcp/decorators/index.ts)、[Resource 源码](https://github.com/rekog-labs/MCP-Nest/blob/main/packages/mcp-nest/src/mcp/decorators/resource.decorator.ts)、[ResourceTemplate 源码](https://github.com/rekog-labs/MCP-Nest/blob/main/packages/mcp-nest/src/mcp/decorators/resource-template.decorator.ts)、[Prompts 指南](https://github.com/rekog-labs/MCP-Nest/blob/main/docs/prompts.md)。模板装饰器的配置字段为 `uriTemplate`，资源装饰器为 `uri`。
- 官方 Python v1：[v1.x FastMCP 源码](https://github.com/modelcontextprotocol/python-sdk/blob/v1.x/src/mcp/server/fastmcp/server.py)；v2：[迁移说明中保持不变的高层 API](https://py.sdk.modelcontextprotocol.io/migration/#what-is-unchanged-on-mcpserver)、[当前 README](https://github.com/modelcontextprotocol/python-sdk)。
- 官方 TypeScript：[Tools](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/docs/servers/tools.md)、[Resources](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/docs/servers/resources.md)、[Prompts](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/docs/servers/prompts.md)。此处 `ResourceTemplate` 是构造对象的类名，并非装饰器。

MCP-Nest 当前还要求能力方法所在的类使用 `@McpController()`，并放进 Nest 模块的 `controllers`，通过 `McpStrategy` 启动；早期 `McpModule.forRoot()` 示例不能与当前代码混用。见 [服务启动示例](https://github.com/rekog-labs/MCP-Nest/blob/main/docs/server-examples.md)。

## 4. 功能职责如何划分

| 关注点 | 已核实的外部做法 | 对本项目的设计判断 |
| --- | --- | --- |
| 能力声明 | FastMCP 保持 tool/resource/prompt 三种声明；MCP-Nest 将固定资源和资源模板分别声明。见 [FastMCP 概览](https://gofastmcp.com/servers/server)、[MCP-Nest 装饰器导出](https://github.com/rekog-labs/MCP-Nest/blob/main/packages/mcp-nest/src/mcp/decorators/index.ts)。 | 保留 `@Tool`、`@Resource`、`@Prompt`；模板放在 `@Resource` 配置中。 |
| 参数与结果 | MCP-Nest 的 Tool 配置包含 `parameters`、`outputSchema`、`annotations`，当前源码也允许 Zod、Standard Schema、原始 JSON Schema。Prompt 返回消息；Resource 返回内容。见 [Tool 源码](https://github.com/rekog-labs/MCP-Nest/blob/main/packages/mcp-nest/src/mcp/decorators/tool.decorator.ts)、[Prompts](https://github.com/rekog-labs/MCP-Nest/blob/main/docs/prompts.md)、[Resources](https://github.com/rekog-labs/MCP-Nest/blob/main/docs/resources.md)。 | 三种能力分别建模；输入输出对象 Schema 是 Tool 合同的一部分，URI 模板参数与 Prompt 参数应有自己的声明。 |
| DI 与请求上下文 | FastMCP 支持 `Depends()`、`CurrentContext()`；MCP-Nest 使用 Nest DI，并通过 `@Payload()`、`@Ctx()` 获取调用参数与上下文。见 [FastMCP Tools](https://gofastmcp.com/servers/tools)、[Context](https://gofastmcp.com/servers/context)、[MCP-Nest Tools](https://github.com/rekog-labs/MCP-Nest/blob/main/docs/tools.md)。 | Angular 负责创建和注入业务实例；插件负责绑定其能力声明与请求上下文。 |
| 进度、取消与日志 | FastMCP 通过 Context 提供 `report_progress` 等能力；MCP-Nest 通过 `McpContext.reportProgress()` 报告进度。官方 TS SDK 将进度、日志、取消放在处理器上下文与协议基础设施中。见 [FastMCP Context](https://gofastmcp.com/servers/context)、[MCP-Nest Tools](https://github.com/rekog-labs/MCP-Nest/blob/main/docs/tools.md)、[官方 TS 指南](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/docs/servers/logging-progress-cancellation.md)。 | 采用上下文方法与 `AbortSignal`，无需新增能力装饰器。 |
| 认证和能力权限 | FastMCP 用组件 `auth` 检查及 `AuthMiddleware`；MCP-Nest 用认证 guard，以及 `@PublicTool`、`@ToolScopes`、`@ToolRoles` 过滤工具列表和检查调用。见 [FastMCP Authorization](https://gofastmcp.com/servers/authorization)、[MCP-Nest Per-Tool Authorization](https://github.com/rekog-labs/MCP-Nest/blob/main/docs/per-tool-authorization.md)。 | 在能力配置中声明 policy，由 Rust 调用管线执行权限检查；保持前端装饰器集合简短。 |
| 列表变化与资源更新 | 官方 TS SDK 的注册句柄支持更新、启停、移除，并发送列表变更通知；资源内容更新有独立通知入口。见 [Notifications](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/docs/servers/notifications.md)。 | 区分“有哪些资源发生变化”与“某个 URI 的内容发生变化”；通过注册句柄/上下文操作表达。 |

一个需要注意的差异：MCP-Nest 文档说明，动态注册处理器直接由 strategy 调用，不经过 Nest RPC 的 guards/pipes/interceptors/filters；其静态装饰器入口则经过这条管线。我们应让装饰器与函数式注册汇合到统一的权限、校验与调用管线。见 [MCP-Nest Dynamic Capabilities](https://github.com/rekog-labs/MCP-Nest/blob/main/docs/dynamic-capabilities.md)。后一句是本项目的设计判断。

## 5. 本项目建议

建议对外保持四个单词装饰器：

| 装饰器 | 本项目职责 | 当前状态 |
| --- | --- | --- |
| `@Module` | 组织能力模块与命名空间 | 已实现 |
| `@Tool` | 声明工具合同与执行策略 | 已实现基础版 |
| `@Resource` | 声明固定 URI 资源或 URI 模板资源 | 尚未实现 |
| `@Prompt` | 声明参数化消息模板 | 尚未实现 |

其中 `@Module` 是本框架自己的组织方式；MCP 协议不会规定 TypeScript 装饰器叫什么。这里借鉴 FastMCP 的三类能力命名、统一资源声明，以及 MCP-Nest 的方法元数据和宿主 DI 绑定；Tauri 的传输和原生生命周期仍由 Rust 层承担。

`@Resource` 建议显式采用 `uri` / `uriTemplate` 互斥配置，而不是仅依赖 URI 字符串推断。这是本项目的 API 选择：借鉴的是合并入口，内部仍需要分别实现资源列表、模板列表和 URI 读取。`@Prompt` 返回消息，模型生成能力由调用方负责；它的业务名称可以是 `summarize`、`review` 等，不需要另造一种能力类别。

订阅实现必须按本项目支持的协议版本验收。现有设计以 `2025-11-25` 为已验证基线；`2026-07-28` 使用 `subscriptions/listen` 承载变更通知，不能直接套用旧订阅流程。见 [本项目设计](mcp-framework-design.md)、[官方 TS 通知的版本说明](https://github.com/modelcontextprotocol/typescript-sdk/blob/main/docs/servers/notifications.md#publish-a-resource-update-through-the-handler)。本次调研不宣称本项目已经支持新协议或资源订阅。
