# 设计思想

此工具首先会解析 `libqiniu_ng.h` 并将代码转换为抽象语法树（`ast` 模块）。
然后，调用指定的语言的翻译模块，将通用的抽象语法树转换为特定语言的抽象语法树，最后遍历语法树并获得最终代码。

## 模块设计

### 功能模块

| 模块名                                                       | 模块描述                                                     |
| ------------------------------------------------------------ | ------------------------------------------------------------ |
| main                          | 可执行程序入口代码，解析参数并调用指定的代码实现功能 |
| ast                           | 解析 C 语言头文件代码并将其转换为抽象语法树 |
| ruby                          | 将 `ast` 生成的抽象语法树转换为 Ruby 绑定代码 |
| dump_entity                   | 将 `ast` 生成的抽象语法树展示出来，仅用于功能调试 |
| classifier                    | 将相同类的方法归类，并从中辨识出构造函数和析构函数 |
| utils                         | 为方便各个语言编写自身的模块而抽象出来的通用库 |

## Ruby 模块

Ruby 模块所有生成代码基于 [FFI](https://rubygems.org/gems/ffi) 库实现。使用该库可以获得更好的内存安全特性及其无缝对接 JRuby。

默认情况下，Ruby 模块将会将代码生成在 `QiniuNg::Bindings` 模块内。因此，生成代码的文件应该配置在 `qiniu-ruby/<GEM PATH>/lib/qiniu_ng/bindings.rb` 路径上。

与 C 接口的绑定代码总是生成在 `QiniuNg::Bindings::CoreFFI` 模块内，包含结构体，枚举类和关联函数，该模块是私有模块，只能被 `QiniuNg::Bindings` 调用。

而 `QiniuNg::Bindings` 模块将面向过程的 `QiniuNg::Bindings::CoreFFI` 内方法转换为面向对象，提供内存安全的保障。

### 功能模块

| 模块名                                                       | 模块描述                                                     |
| ------------------------------------------------------------ | ------------------------------------------------------------ |
| mod                           | Ruby 模块入口，负责遍历外层的 AST 并调用代码生成模块 |
| ast                           | Ruby 代码抽象语法树 |
| types                         | Ruby 类型枚举类，可以将 Clang 解析得到的类型翻译为 Ruby 类型 |
| utils                         | 为方便 Ruby 类型判断和标识符转换而抽象出来的通用库 |
| ffi_bindings                         | 负责为模块插入 FFI 初始化语句 |
| callback_declaration_bindings        | 负责为模块插入回调类型声明语句，该模块为遍历所有绑定的类型和方法，从中发现回调类型，并转换成 Ruby 的回调类型声明 |
| type_declaration_bindings            | 负责为模块插入类型声明语句 |
| attach_function_declaration_bindings | 负责为模块插入方法声明语句 |
| ffi_wrapper_classes                  | 负责为模块插入面向对象的 FFI 封装类型 |
