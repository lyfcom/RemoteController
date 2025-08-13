fn main() {
    // 尝试在 MSVC 上生成更小的可执行文件的建议，具体静态链接需在安装 MSVC CRT 静态库时配置
    // 如果使用 GNU toolchain，可在 .cargo/config.toml 中指定目标为 x86_64-pc-windows-gnu 并启用 -static
    #[cfg(windows)]
    {
        // 让控制台隐藏可通过链接子系统处理，这里保持默认控制台，便于调试
        println!("cargo:rerun-if-changed=build.rs");
    }
}
