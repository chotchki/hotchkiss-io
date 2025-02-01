/// A wrapper type used to pass the navigation tabs and which one is active
pub struct TopBar(pub Vec<(String, bool)>);

impl TopBar {
    pub fn new(pages: Vec<String>) -> Self {
        Self(pages.into_iter().map(|p| (p, false)).collect())
    }

    pub fn make_active(mut self, page: &str) -> Self {
        for (p, a) in self.0.iter_mut() {
            *a = page == p
        }

        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activate() {
        let tb = TopBar::new(vec!["Foo".to_string(), "Bar".to_string()]);

        assert_eq!(
            tb.0,
            vec![("Foo".to_string(), false), ("Bar".to_string(), false)]
        );

        let tb = tb.make_active("Bar");

        assert_eq!(
            tb.0,
            vec![("Foo".to_string(), false), ("Bar".to_string(), true)]
        );
    }
}
