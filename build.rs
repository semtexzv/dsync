fn  main() {
    let dotenv_path = dotenvy::dotenv().expect("failed to find .env file");
    println!("cargo:rerun-if-changed={}", dotenv_path.display());

    // Warning: `dotenv_iter()` is deprecated! Roll your own or use a maintained fork such as `dotenvy`.
    for env_var in dotenvy::dotenv_iter().unwrap() {
        let (key, value) = env_var.unwrap();
        println!("cargo:rustc-env={key}={value}");
    }
}
