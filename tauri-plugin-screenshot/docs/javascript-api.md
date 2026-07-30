# JavaScript API Reference

`tauri-plugin-screenshot-api` 提供 Tauri 原生区域截图的 JavaScript Guest bindings。

```ts
import { captureArea } from 'tauri-plugin-screenshot-api'
```

## Interfaces

### `CaptureOptions`

区域截图选项。

| Property | Type | Default | Description |
| --- | --- | --- | --- |
| `minWidth?` | `number` | `2` | 最小选区宽度，逻辑像素 |
| `minHeight?` | `number` | `2` | 最小选区高度，逻辑像素 |

传入值应为非负整数；`0` 会按 `1` 逻辑像素处理。

### `CaptureRegion`

选区在冻结的虚拟桌面图像中的位置。所有字段均为物理像素。

| Property | Type | Description |
| --- | --- | --- |
| `x` | `number` | 相对于虚拟桌面图像左上角的 X 偏移 |
| `y` | `number` | 相对于虚拟桌面图像左上角的 Y 偏移 |
| `width` | `number` | 选区宽度 |
| `height` | `number` | 选区高度 |

### `CapturedScreenshot`

用户确认截图后返回的结果。

| Property | Type | Description |
| --- | --- | --- |
| `status` | `'captured'` | 可辨识联合标记 |
| `mimeType` | `'image/png'` | 输出 MIME 类型 |
| `width` | `number` | PNG 宽度，物理像素 |
| `height` | `number` | PNG 高度，物理像素 |
| `region` | [`CaptureRegion`](#captureregion) | 选区在虚拟桌面图像中的位置 |
| `data` | `Uint8Array` | PNG 原始字节 |

### `CancelledScreenshot`

用户取消截图后返回的结果。

| Property | Type | Description |
| --- | --- | --- |
| `status` | `'cancelled'` | 可辨识联合标记 |

## Type Aliases

### `CaptureResult`

```ts
type CaptureResult = CapturedScreenshot | CancelledScreenshot
```

使用 `status` 缩小类型：

```ts
const result = await captureArea()

if (result.status === 'captured') {
  console.log(result.data)
}
```

## Functions

### `captureArea()`

```ts
function captureArea(options?: CaptureOptions): Promise<CaptureResult>
```

启动一次原生区域截图。Promise 会在用户确认或取消后完成；同一时间只能存在一个截图会话。

#### Parameters

| Parameter | Type | Default | Description |
| --- | --- | --- | --- |
| `options` | [`CaptureOptions`](#captureoptions) | `{}` | 可选的最小选区配置 |

#### Returns

`Promise<CaptureResult>`：确认时返回 PNG 字节和区域信息，取消时返回 `{ status: 'cancelled' }`。

#### Example

```ts
import { captureArea } from 'tauri-plugin-screenshot-api'

const result = await captureArea({ minWidth: 8, minHeight: 8 })

if (result.status === 'captured') {
  const blob = new Blob([result.data], { type: result.mimeType })
  const url = URL.createObjectURL(blob)
  console.log(result.region, url)
}
```

#### Errors

失败时 Promise 会以 `{ code, message, recoverable }` 结构拒绝。用户主动取消不属于错误。错误码与处理建议见主文档的 [Errors](../README.md#errors)。

#### Permissions

需要 `screenshot:allow-capture-area` 和 `screenshot:allow-take-capture`，也可以直接启用包含二者的 `screenshot:default`。

#### Since

`0.1.0`

#### Source

[`guest-js/index.ts`](../guest-js/index.ts)
