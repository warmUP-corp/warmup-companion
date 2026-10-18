pub fn sddl_from_sdshow(stdout: &str) -> Option<String> {
    stdout.lines().map(str::trim).find_map(|line| {
        if line.starts_with("D:") || line.starts_with("O:") || line.starts_with("G:") {
            Some(line.to_string())
        } else {
            None
        }
    })
}

pub fn ensure_iu_rp_wp(sddl: &str) -> String {
    let trimmed = sddl.trim();
    if let Some((start, end, rights)) = find_iu_allow_ace(trimmed) {
        let next_rights = with_rp_wp(&rights);
        if next_rights == rights {
            return trimmed.to_string();
        }
        let inner = &trimmed[start + 1..end - 1];
        let parts: Vec<&str> = inner.split(';').collect();
        if parts.len() != 6 {
            return trimmed.to_string();
        }
        let new_ace = format!(
            "({};{};{};{};{};{})",
            parts[0], parts[1], next_rights, parts[3], parts[4], parts[5]
        );
        let mut out = String::with_capacity(trimmed.len() + 4);
        out.push_str(&trimmed[..start]);
        out.push_str(&new_ace);
        out.push_str(&trimmed[end..]);
        return out;
    }
    let ace = "(A;;CCLCSWRPWPLOCRRC;;;IU)";
    if let Some(sacl) = sacl_start(trimmed) {
        let mut out = String::with_capacity(trimmed.len() + ace.len());
        out.push_str(&trimmed[..sacl]);
        out.push_str(ace);
        out.push_str(&trimmed[sacl..]);
        out
    } else {
        format!("{trimmed}{ace}")
    }
}

fn find_iu_allow_ace(sddl: &str) -> Option<(usize, usize, String)> {
    let bytes = sddl.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'(' {
            i += 1;
            continue;
        }
        let Some(rel) = sddl[i + 1..].find(')') else {
            break;
        };
        let end = i + 1 + rel + 1;
        let inner = &sddl[i + 1..end - 1];
        let parts: Vec<&str> = inner.split(';').collect();
        if parts.len() == 6 && parts[0] == "A" && parts[5] == "IU" {
            return Some((i, end, parts[2].to_string()));
        }
        i = end;
    }
    None
}

fn sacl_start(sddl: &str) -> Option<usize> {
    let bytes = sddl.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i + 1 < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b'S' if depth == 0 && bytes[i + 1] == b':' => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

fn with_rp_wp(rights: &str) -> String {
    let mut pairs: Vec<&str> = rights
        .as_bytes()
        .chunks(2)
        .filter_map(|chunk| std::str::from_utf8(chunk).ok())
        .collect();
    if !pairs.iter().any(|p| *p == "RP") {
        if let Some(i) = pairs.iter().position(|p| *p == "SW") {
            pairs.insert(i + 1, "RP");
        } else {
            pairs.push("RP");
        }
    }
    if !pairs.iter().any(|p| *p == "WP") {
        if let Some(i) = pairs.iter().position(|p| *p == "RP") {
            pairs.insert(i + 1, "WP");
        } else {
            pairs.push("WP");
        }
    }
    pairs.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_IU: &str = "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)";
    const GRANTED_IU: &str = "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWRPWPLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)";

    #[test]
    fn sdshow_picks_the_sddl_line() {
        let out = "\n[SC] GetServiceSecurity SUCCESS\n\nD:(A;;CCLCSWLOCRRC;;;IU)\n";
        assert_eq!(
            sddl_from_sdshow(out).as_deref(),
            Some("D:(A;;CCLCSWLOCRRC;;;IU)")
        );
        assert_eq!(sddl_from_sdshow("no descriptor here"), None);
    }

    #[test]
    fn default_service_sddl_gains_iu_rp_and_wp() {
        assert_eq!(ensure_iu_rp_wp(DEFAULT_IU), GRANTED_IU);
    }

    #[test]
    fn already_granted_sddl_is_unchanged() {
        assert_eq!(ensure_iu_rp_wp(GRANTED_IU), GRANTED_IU);
        assert_eq!(ensure_iu_rp_wp(&ensure_iu_rp_wp(DEFAULT_IU)), GRANTED_IU);
    }

    #[test]
    fn missing_iu_ace_is_appended_before_sacl() {
        let input = "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)S:(AU;FA;KA;;;WD)";
        let out = ensure_iu_rp_wp(input);
        assert!(out.contains("(A;;CCLCSWRPWPLOCRRC;;;IU)"));
        assert!(out.ends_with("S:(AU;FA;KA;;;WD)"));
        assert!(out.starts_with("D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)"));
    }

    #[test]
    fn missing_iu_ace_is_appended_when_no_sacl() {
        let input = "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)";
        assert_eq!(
            ensure_iu_rp_wp(input),
            "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCLCSWRPWPLOCRRC;;;IU)"
        );
    }

    #[test]
    fn rp_only_iu_ace_gains_wp() {
        let input = "D:(A;;CCLCSWRPLOCRRC;;;IU)";
        assert_eq!(ensure_iu_rp_wp(input), "D:(A;;CCLCSWRPWPLOCRRC;;;IU)");
    }

    #[test]
    fn other_aces_are_preserved() {
        let out = ensure_iu_rp_wp(DEFAULT_IU);
        assert!(out.contains("(A;;CCLCSWRPWPDTLOCRRC;;;SY)"));
        assert!(out.contains("(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)"));
        assert!(out.contains("(A;;CCLCSWLOCRRC;;;SU)"));
        assert!(!out.contains("(A;;CCLCSWLOCRRC;;;IU)"));
    }
}
