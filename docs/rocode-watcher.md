# rocode-watcher

`rocode-watcher` 提供文件系统监听与事件广播能力。

## 主要职责

- 监听目录文件变化
- 对事件做忽略规则过滤与去抖
- 广播标准化变更事件

## 核心类型

- `FileWatcher`
- `WatcherConfig`
- `WatcherEvent`
- `FileEvent`
- `WatcherError`

## 默认行为

- 默认递归监听
- 默认忽略：`.git`、`node_modules`、`target`、临时文件
- 默认 debounce：`100ms`

## 使用场景

- 会话中的文件变更通知
- 工具链的上下文刷新
- 侧边状态和诊断信息更新

## 验证

```bash
cargo check -p rocode-watcher
```
