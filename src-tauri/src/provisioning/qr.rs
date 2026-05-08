use qrcode::QrCode;

use crate::provisioning::error::ProvisioningError;

/// Generate an SVG string containing a QR code for the given provisioning URL.
///
/// The URL is expected to be in the format:
/// `sgnl://linkdevice?uuid=<UUID>&pub_key=<BASE64_PUBKEY>`
///
/// The SVG uses a dark background with light modules to match our dark theme.
pub fn generate_qr_svg(url: &str) -> Result<String, ProvisioningError> {
    let code = QrCode::new(url.as_bytes())
        .map_err(|e| ProvisioningError::QrGenerationFailed(e.to_string()))?;

    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(256, 256)
        .dark_color(qrcode::render::svg::Color("#ffffff"))
        .light_color(qrcode::render::svg::Color("#1a1a2e"))
        .quiet_zone(true)
        .build();

    Ok(svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_valid_svg() {
        let url = "sgnl://linkdevice?uuid=test-uuid&pub_key=dGVzdC1rZXk";
        let svg = generate_qr_svg(url).unwrap();

        assert!(svg.starts_with("<?xml"));
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
    }

    #[test]
    fn svg_uses_correct_colors() {
        let url = "sgnl://linkdevice?uuid=test-uuid&pub_key=dGVzdC1rZXk";
        let svg = generate_qr_svg(url).unwrap();

        // Dark modules should be white (for dark theme)
        assert!(svg.contains("#ffffff"));
        // Light modules should be dark background
        assert!(svg.contains("#1a1a2e"));
    }

    #[test]
    fn generates_different_qr_for_different_urls() {
        let svg1 = generate_qr_svg("sgnl://linkdevice?uuid=aaa&pub_key=bbb").unwrap();
        let svg2 = generate_qr_svg("sgnl://linkdevice?uuid=ccc&pub_key=ddd").unwrap();

        assert_ne!(svg1, svg2);
    }

    #[test]
    fn handles_empty_input() {
        // QR codes can encode empty strings, so this should succeed
        let result = generate_qr_svg("");
        assert!(result.is_ok());
    }
}
