# Tauri MCP 服务框架

本项目为 Tauri 应用提供发布业务能力的 MCP 服务框架。业务模块声明 Tool，框架管理其对外契约、可用性和调用过程。

## Language

**MCP 应用服务**：
属于一个应用、向 MCP Client 提供业务能力的服务。服务的身份与业务能力属于应用。
_Avoid_: 聊天机器人、模型 Provider

**Capability**：
应用向调用方提供的业务能力，包括 Tool、Resource 与 Prompt。
_Avoid_: MCP 协议 capability 标志、宿主权限

**CapabilityManifest**：
用于登记的一组能力声明，包含对外契约及其所需的执行规则。
_Avoid_: 业务实例、处理方法、发现结果

**CapabilityBinding**：
一个 Capability 与其业务处理方法及实例之间的绑定关系。
_Avoid_: CapabilityManifest、能力的公开名称

**ExecutionHost**：
承载能力处理方法、提供其执行可用性的运行环境。
_Avoid_: Caller、MCP Client、业务对象

**Tool**：
具有稳定名称、明确输入输出和行为说明的可调用业务能力。
_Avoid_: 任意方法、通用执行入口

**Resource**：
通过 URI 标识、供客户端读取并用作模型上下文的业务数据。内容可以在读取时生成。
_Avoid_: Tool、业务操作、文件路径

**ResourceTemplate**：
描述一类参数化 Resource 的 URI 模板。客户端用具体参数展开 URI 后读取对应 Resource。
_Avoid_: Prompt、Tool 输入 Schema

**Prompt**：
可被发现、按参数获取的一组消息或指令模板，用于引导模型处理业务任务。
_Avoid_: 模型推理、ToolInvocation、业务工作流执行

**ToolDescriptor**：
向 MCP Client 发布的 Tool 契约，描述名称、用途、数据结构和行为提示。
_Avoid_: ToolPolicy、Handler

**ToolPolicy**：
宿主实际执行的 Tool 访问与调用规则，包括所需权限、时间预算和并发约束。
_Avoid_: 行为提示、授权结果

**ToolProvider**：
提供一组 Tool 及其业务处理方法的模块。
_Avoid_: MCP Client、Caller

**ToolBinding**：
一个 Tool 与其业务处理方法及实例之间的绑定关系。
_Avoid_: ToolDescriptor、协议方法

**ToolRegistration**：
ToolProvider 向框架发布一组 Tool 的注册关系，具有明确的有效期和释放方式。
_Avoid_: 导入模块、一次调用

**ToolInvocation**：
对一个已注册 Tool 发起的一次调用尝试，包含调用者、输入和调用结果。
_Avoid_: 业务事务、持久化任务、幂等操作

**ToolResult**：
一次 ToolInvocation 对外返回的业务数据或受控执行错误。
_Avoid_: HTTP 响应、JSON-RPC 协议错误

**Caller**：
由宿主认证确认、接受业务授权检查的调用主体。同一 Caller 可以通过多个 MCP Client 发起调用。
_Avoid_: 客户端名称、连接标识

**MCP Client**：
通过 MCP 发现能力并发起请求的外部应用。
_Avoid_: Caller、业务用户

**ToolCatalog**：
某个 Caller 在当前服务状态和授权规则下可以发现的 ToolDescriptor 集合。
_Avoid_: 所有业务方法、调用历史

**RegistryRevision**：
一次已提交的 Tool 注册状态的版本，用于识别 ToolCatalog 所依据的快照。
_Avoid_: ToolContractVersion、MCPProtocolVersion

**ToolContractVersion**：
一个 Tool 的输入、输出和业务语义契约的版本。
_Avoid_: RegistryRevision、包版本

**MCPProtocolVersion**：
MCP 消息、传输和功能协商所依据的协议版本。
_Avoid_: 内部桥接版本、ToolContractVersion
