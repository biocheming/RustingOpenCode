use chrono::{DateTime, Utc};

fn resolve_build_date() -> String {
    if let Ok(raw) = std::env::var("SOURCE_DATE_EPOCH") {
        if let Ok(epoch) = raw.parse::<i64>() {
            if let Some(ts) = DateTime::<Utc>::from_timestamp(epoch, 0) {
                return ts.format("%Y.%m.%d").to_string();
            }
        }
        println!(
            "cargo:warning=Invalid SOURCE_DATE_EPOCH ('{}'), falling back to current UTC date",
            raw
        );
    }

    Utc::now().format("%Y.%m.%d").to_string()
}

fn main() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rustc-env=ROCODE_BUILD_DATE={}", resolve_build_date());
}
