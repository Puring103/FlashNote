# gpui-component 本地补丁

本目录基于 crates.io 的 gpui-component 0.5.1，按上游 Apache-2.0 许可证保留源码与许可证。项目通过 Cargo 的 [patch.crates-io] 使用此版本。

索笺只调整了 `src/menu/popup_menu.rs` 中的菜单容器样式：使用主题提供的纸面底色和细边框，移除上游默认的大面积阴影。更新该依赖时请重新检查此补丁与真实窗口中的菜单层级。
