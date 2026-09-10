pub use chrono::{DateTime, Utc};

#[cfg(test)]
mod tests {
    use super::{DateTime, Utc};

    #[test]
    fn reexports_utc_datetime() {
        let now: DateTime<Utc> = Utc::now();
        assert_eq!(now.timezone(), Utc);
    }
}
