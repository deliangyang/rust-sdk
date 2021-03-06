# 通用 Qiniu SDK

[![License](https://img.shields.io/badge/license-Apache%202-blue)](https://github.com/bachue/rust-sdk/blob/master/LICENSE)[![Build Status](https://api.travis-ci.com/bachue/rust-sdk.svg?branch=master)](https://travis-ci.org/bachue/rust-sdk)

## 关于

此通用 SDK 基于 [七牛云官方 API](http://developer.qiniu.com/) 构建。
使用此 SDK 构建您的网络应用程序，能让您以非常便捷地方式将数据安全地存储到七牛云上。
无论您的网络应用是一个网站程序，还是包括从云端（服务端程序）到终端（手持设备应用）的架构的服务或应用，通过七牛云及其 SDK，都能让您应用程序的终端用户高速上传和下载，同时也让您的服务端更加轻盈。

## 兼容平台

- Linux
- Windows
- MacOS

## 依赖环境

- Rust 1.38+
- 其他依赖参见各个模块中的 README.md

## 构建指南

### 构建所有库的开发版

```bash
make
```

### 构建所有库的发布版

```bash
make build_release
```

### 生成 API 文档

```bash
make doc
```

### 测试所有库（需要配置七牛账户，具体做法参见 [TEST.md](TEST.md)）

```bash
make test
```

### 检查 Rust 代码质量

```bash
make clippy
```

### 删除构建结果

```bash
make clean
```

## 测试指南

参见 [TEST.md](TEST.md)

## 设计文档

参见 [DESIGN.md](DESIGN.md)

## 贡献代码

1. Fork
2. 创建您的特性分支 (`git checkout -b my-new-feature`)
3. 提交您的改动 (`git commit -am 'Added some feature'`)
4. 将您的修改记录提交到远程 `git` 仓库 (`git push origin my-new-feature`)
5. 然后到 github 网站的该 `git` 远程仓库的 `my-new-feature` 分支下发起 Pull Request

## 许可证

Copyright (c) 2012-2020 qiniu.com

基于 Apache 2.0 协议发布:

* [opensource.org/licenses/Apache-2.0](https://opensource.org/licenses/Apache-2.0)
