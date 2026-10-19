fn main() {
    // `check.rs` 用 `option_env!("WINSSH_UPDATE_ENDPOINT")` 在编译期定接口
    // 地址；只改环境变量不重编的话，跑起来的还是旧地址。
    println!("cargo:rerun-if-env-changed=WINSSH_UPDATE_ENDPOINT");
}
