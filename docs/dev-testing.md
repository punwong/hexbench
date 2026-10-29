# 开发/测试用的固定环境

> 目的：自动化测试和手动验证都用同一套目录，**不要再动用户配置里的 `root` / `folders`**。

## 目录

```
D:\hexbench-test\                 ← 启动器根目录（每个子文件夹 = 一个分类）
├── 文档\          文档1..5.txt
├── 图片\          图片1.png / 图片2.png（用于验证缩略图）
├── 项目\          代码1..3.txt + 子模块\深层文件.txt（用于验证进层/返回）
└── 临时文件\      临时1..2.log
```

当前配置（`%APPDATA%\io.github.hexbench\config.json`）：

| 字段 | 值 |
| --- | --- |
| `launcher.root` | `D:\hexbench-test` |
| `launcher.folders` | `D:\hexbench-test\文档`、`D:\hexbench-test\项目`、`%USERPROFILE%\Downloads` |

## 测试规则（重要）

1. **不要清空 `root` / `folders`**：用户会用它手动验证，清掉就得重新添加。
2. 需要临时换根目录时：先备份 `config.json`，测完立刻还原。
3. 需要「空目录」场景：在 `D:\hexbench-test\` 下临时建一个空子文件夹当分类，测完删掉即可，**不要改 root**。
4. 造数据（新建/复制/删除的验证）一律在 `D:\hexbench-test\` 里做，别碰用户的真实文件。

## 自动化通道

debug 构建下有个自测入口：放一个文件

```
%TEMP%\hexbench-selftest.txt
```

启动应用后会自动跑一组操作，把结果写到 `%TEMP%\hexbench-selftest-out.txt`。
它比截图可靠——直接读真实的窗口尺寸和可见性。

## 验证常用手法

| 目的 | 手法 |
| --- | --- |
| 快捷键是否触发 | 看日志里的 `clipboard:` / `trash:` / `bg menu:` 行 |
| 文件操作是否成功 | 直接 `Get-ChildItem` 看磁盘 |
| 窗口尺寸/位置 | 枚举窗口 + `GetWindowRect` |
| 刷新是否及时 | 操作后截图看列表，或读日志 |
