use tracing::metadata::LevelFilter;
use tracing_subscriber::fmt::time::LocalTime;
use tracing_subscriber::EnvFilter;

pub fn tracing_init() {
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    let time_format = time::format_description::parse(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3][offset_hour sign:mandatory]:[offset_minute]",
    )
        .unwrap();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_timer(LocalTime::new(time_format))
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}
