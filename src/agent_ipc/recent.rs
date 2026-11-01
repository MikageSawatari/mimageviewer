//! Ephemeral, bounded self-reported identities shared with the preferences UI.
use super::*;
use std::time::SystemTime;

pub(crate) type SharedRecentApps = Arc<Mutex<RecentApps>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecentApp {
    pub(crate) client_name: String,
    pub(crate) client_version: String,
    pub(crate) last_called: SystemTime,
}

#[derive(Default)]
pub(crate) struct RecentApps {
    entries: Vec<RecentApp>,
}
impl RecentApps {
    /// Most recently called first. Reading the table never changes its order.
    pub(crate) fn entries(&self) -> &[RecentApp] {
        &self.entries
    }

    fn record_at(&mut self, client: &ClientIdentity, at: SystemTime) {
        let client = ClientIdentity::new(&client.name, &client.version);
        self.entries
            .retain(|old| old.client_name != client.name || old.client_version != client.version);
        self.entries.insert(
            0,
            RecentApp {
                client_name: client.name,
                client_version: client.version,
                last_called: at,
            },
        );
        self.entries.truncate(8);
    }

    pub(super) fn record(&mut self, client: &ClientIdentity) {
        self.record_at(client, SystemTime::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_apps_are_bounded_lru_by_name_and_version() {
        let mut recent = RecentApps::default();
        let at = SystemTime::UNIX_EPOCH;
        for n in 0..8 {
            recent.record_at(&ClientIdentity::new(&format!("app{n}"), "1"), at);
        }
        recent.record_at(
            &ClientIdentity::new("app0", "1"),
            at + Duration::from_secs(1),
        );
        assert_eq!(recent.entries().len(), 8);
        assert_eq!(recent.entries()[0].client_name, "app0");
        assert_eq!(recent.entries()[0].last_called, at + Duration::from_secs(1));
        recent.record_at(
            &ClientIdentity::new("app0", "2"),
            at + Duration::from_secs(2),
        );
        assert_eq!(recent.entries().len(), 8);
        assert!(!recent.entries().iter().any(|e| e.client_name == "app1"));
        assert_eq!(recent.entries()[0].client_version, "2");
        assert_eq!(recent.entries()[1].client_version, "1");
    }

    #[test]
    fn recent_apps_truncate_unicode_by_characters_at_the_owner_boundary() {
        let mut recent = RecentApps::default();
        recent.record_at(
            &ClientIdentity {
                name: "名".repeat(140),
                version: "版".repeat(80),
            },
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(recent.entries()[0].client_name.chars().count(), 128);
        assert_eq!(recent.entries()[0].client_version.chars().count(), 64);
    }
}
