//! Per-book presentation state. Quote tracking never mutates the native book.
pub(crate) struct Camera {
    pub center: f32,
    pub span: f32,
    pub centered: bool,
    pub quote_midpoint: f64,
    pub epoch: u32,
    pub frozen_at: Option<f64>,
    panned: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center: 0.0,
            span: 1.0,
            centered: false,
            quote_midpoint: 0.0,
            epoch: 1,
            frozen_at: None,
            panned: false,
        }
    }
}

impl Camera {
    pub fn minimum(&self) -> f32 {
        self.center - self.span / 2.0
    }

    pub fn reset_range(&mut self, fraction: f32) {
        // Keep bins representable even if a caller requests a sub-f32 range.
        self.span = (f64::from(self.center.abs().max(1e-20))
            * f64::from(fraction.max(16.0 * f32::EPSILON)))
        .min(f64::from(f32::MAX)) as f32;
    }

    pub fn recenter(&mut self) {
        self.centered = false;
        self.panned = false;
    }

    pub fn invalidate(&mut self) {
        self.epoch += 1;
        self.recenter();
    }

    pub fn pan(&mut self, fraction: f32) {
        self.center = (self.center + self.span * fraction).max(f32::MIN_POSITIVE);
        self.centered = true;
        self.panned = true;
    }

    /// Fit both quotes, including a wide spread or a book with only one side.
    /// Grow with headroom and never shrink until an explicit range/recenter.
    /// Return whether GPU camera parameters changed. Raster history keeps its
    /// epoch: each captured column carries its original price coordinates.
    pub fn update(
        &mut self,
        bid: Option<f64>,
        ask: Option<f64>,
        fraction: f32,
        warming: bool,
    ) -> bool {
        let valid = |price: &f64| *price > 0.0 && (*price as f32).is_finite();
        let bid = bid.filter(valid);
        let ask = ask.filter(valid);
        self.quote_midpoint = crate::presentation::midpoint(bid.unwrap_or(0.0), ask.unwrap_or(0.0));
        let (low, high) = match (bid, ask) {
            (Some(b), Some(a)) => (b.min(a), b.max(a)),
            (Some(p), None) | (None, Some(p)) => (p, p),
            (None, None) => return false,
        };
        let mut changed = false;
        if !self.centered || warming {
            self.center = ((low + high) / 2.0) as f32;
            self.reset_range(fraction);
            self.centered = true;
            changed = true;
        }
        if !self.panned {
            let radius = (high - f64::from(self.center))
                .abs()
                .max((low - f64::from(self.center)).abs());
            if radius >= f64::from(self.span) / 2.0 {
                self.span = (radius * 2.5)
                    .max(f64::from(self.span) * 1.25)
                    .min(f64::from(f32::MAX)) as f32;
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::Camera;

    fn camera() -> Camera {
        let mut camera = Camera::default();
        assert!(camera.update(Some(99.99), Some(100.01), 0.005, false));
        camera
    }

    fn contains(camera: &Camera, price: f64) {
        assert!(price > f64::from(camera.minimum()));
        assert!(price < f64::from(camera.minimum() + camera.span));
    }

    #[test]
    fn zooms_out_in_both_directions_with_headroom_without_resetting_history() {
        for quotes in [(100.49, 100.51), (99.49, 99.51), (20.0, 150.0)] {
            let mut camera = camera();
            camera.epoch = 7;
            assert!(camera.update(Some(quotes.0), Some(quotes.1), 0.005, false));
            assert_eq!(camera.center, 100.0);
            assert_eq!(camera.epoch, 7);
            contains(&camera, quotes.0);
            contains(&camera, quotes.1);
            let span = camera.span;
            assert!(!camera.update(Some(quotes.0), Some(quotes.1), 0.005, false));
            assert!(!camera.update(Some(99.99), Some(100.01), 0.005, false));
            assert_eq!(camera.span, span);
        }
    }

    #[test]
    fn fits_the_spread_even_when_midpoint_has_not_moved() {
        let mut camera = camera();
        camera.update(Some(99.0), Some(101.0), 0.005, false);
        contains(&camera, 99.0);
        contains(&camera, 101.0);
        assert_eq!(camera.center, 100.0);
    }

    #[test]
    fn fits_one_sided_and_initially_wide_books() {
        for quotes in [(Some(120.0), None), (None, Some(80.0))] {
            let mut camera = camera();
            assert!(camera.update(quotes.0, quotes.1, 0.005, false));
            contains(&camera, quotes.0.or(quotes.1).unwrap());
        }
        let mut camera = Camera::default();
        camera.update(Some(10.0), Some(100.0), 0.005, false);
        contains(&camera, 10.0);
        contains(&camera, 100.0);
    }

    #[test]
    fn scrolling_pauses_tracking_and_recenter_resumes_it() {
        let mut camera = camera();
        camera.pan(1.0);
        let (center, span) = (camera.center, camera.span);
        assert!(!camera.update(Some(199.99), Some(200.01), 0.005, false));
        assert_eq!((camera.center, camera.span), (center, span));
        camera.recenter();
        assert!(camera.update(Some(199.99), Some(200.01), 0.005, false));
        assert_eq!(camera.center, 200.0);
        assert_eq!(camera.span, 1.0);
        assert!(camera.update(Some(209.99), Some(210.01), 0.005, false));
        contains(&camera, 210.01);
    }

    #[test]
    fn each_book_tracks_its_own_quotes_and_frozen_books_stay_put() {
        let mut moving = camera();
        let mut frozen = camera();
        frozen.frozen_at = Some(12.0);
        moving.update(Some(200.0), Some(201.0), 0.005, false);
        assert!(!frozen.update(Some(99.99), Some(100.01), 0.005, false));
        assert_eq!(frozen.span, 0.5);
        assert_eq!(frozen.frozen_at, Some(12.0));
        assert!(moving.span > frozen.span);
    }

    #[test]
    fn explicit_range_and_feed_invalidation_reset_zoom() {
        let mut camera = camera();
        camera.update(Some(200.0), Some(201.0), 0.005, false);
        camera.reset_range(0.02);
        assert_eq!(camera.span, 2.0);
        camera.pan(1.0);
        camera.invalidate();
        camera.update(Some(299.99), Some(300.01), 0.005, false);
        assert_eq!(camera.center, 300.0);
        assert_eq!(camera.span, 1.5);
        assert_eq!(camera.epoch, 2);
    }

    #[test]
    fn handles_tiny_prices_and_ignores_empty_or_invalid_quotes() {
        let mut camera = Camera::default();
        assert!(!camera.update(None, Some(f64::NAN), 0.005, false));
        camera.update(Some(0.00000001), Some(0.00000002), 0.005, false);
        contains(&camera, 0.00000001);
        contains(&camera, 0.00000002);
        let span = camera.span;
        assert!(!camera.update(Some(f64::INFINITY), Some(0.0), 0.005, false));
        assert_eq!(camera.span, span);
    }
}
