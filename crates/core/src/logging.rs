/// Sends the core's `log` records to logcat under the tag `wirebridge`. Safe to call more than once; does nothing off
/// Android, where the tests install their own logger.
#[uniffi::export]
pub fn init_logging() {
    #[cfg(target_os = "android")]
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("wirebridge"),
    );
}
