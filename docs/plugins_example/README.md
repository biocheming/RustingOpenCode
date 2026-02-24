# plugins_example

这个目录是示例合集，用来回答你这个问题：

- 可以放 `markdown skill` 示例
- 可以放 `TypeScript plugin` 示例
- 也可以放 `Rust` 扩展示例

结论：这个做法是对的，但三者的加载方式不一样。

## 1) Skill (Markdown) 是提示词能力

- 文件格式：`SKILL.md`
- 典型放置目录：`.opencode/skills/<skill-name>/SKILL.md`
- 特点：不改运行时代码，主要给模型注入流程和约束

本目录示例：`docs/plugins_example/skill/SKILL.md`

## 2) TS Plugin 是运行时 Hook/Auth 扩展

- 由 `rocode-plugin` 子进程桥接执行
- 在配置文件里通过 `plugin` 列表声明（兼容路径仍是 `opencode.jsonc`）

示例配置（项目根 `opencode.jsonc`）：

```json
{
  "plugin": [
    "file:///ABS/PATH/TO/docs/plugins_example/ts/example-plugin.ts"
  ]
}
```

本目录示例：`docs/plugins_example/ts/example-plugin.ts`

## 3) Rust 示例是编译期扩展

- Rust 代码不会像 TS 插件那样被动态 `import`
- 需要你在 Rust 工程里显式注册并重新编译

本目录示例：`docs/plugins_example/rust/example_plugin.rs`

## 推荐实践

- 只想增强提示和流程：优先用 Skill
- 需要动态 hook/auth/custom fetch：用 TS Plugin
- 需要深度性能/类型安全/核心能力扩展：改 Rust 代码并编译
