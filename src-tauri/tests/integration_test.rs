#[cfg(feature = "test")]
mod integration {
    use tauri::test::{mock_builder, mock_context, noop_assets};

    #[test]
    fn app_configures_successfully() {
        let _app = mock_builder()
            .build(mock_context(noop_assets()))
            .expect("failed to build app");
    }
}
