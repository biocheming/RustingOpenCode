# rocode-grep

`rocode-grep` 封装文本/文件搜索能力，供工具层与服务层复用。

## 主要职责

- 文件遍历与过滤
- 正则与关键词匹配
- 匹配结果结构化输出
- 搜索统计信息聚合

## 关键导出

- `Ripgrep`
- `FileSearchOptions`
- `MatchResult`
- `SubMatch`
- `Stats`

## 使用场景

- `rocode-tool` 的 grep/codesearch 工具
- 服务端 `/find/*` 相关路由
- 诊断与调试场景中的快速检索

## 开发建议

- 大目录扫描优先做 ignore 过滤
- 结果结构要兼容 TUI 与 JSON 输出
- 保持错误信息可定位（路径、模式、行号）

## 验证

```bash
cargo check -p rocode-grep
```
