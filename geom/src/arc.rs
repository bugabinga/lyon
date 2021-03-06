//! Elliptic arc related maths and tools.

use std::ops::Range;

use Line;
use scalar::{Scalar, Float, cast};
use generic_math::{Point, point, Vector, vector, Rotation2D, Transform2D, Angle, Rect};
use utils::directed_angle;
use segment::{Segment, FlattenedForEach, FlatteningStep, BoundingRect};
use segment;
use QuadraticBezierSegment;

/// A flattening iterator for arc segments.
pub type Flattened<S> = segment::Flattened<S, Arc<S>>;

/// An ellipic arc curve segment using the SVG notation.
#[derive(Copy, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serialization", derive(Serialize, Deserialize))]
pub struct SvgArc<S> {
    pub from: Point<S>,
    pub to: Point<S>,
    pub radii: Vector<S>,
    pub x_rotation: Angle<S>,
    pub flags: ArcFlags,
}

/// An ellipic arc curve segment.
#[derive(Copy, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serialization", derive(Serialize, Deserialize))]
pub struct Arc<S> {
    pub center: Point<S>,
    pub radii: Vector<S>,
    pub start_angle: Angle<S>,
    pub sweep_angle: Angle<S>,
    pub x_rotation: Angle<S>,
}

impl<S: Scalar> Arc<S> {
    pub fn from_svg_arc(arc: &SvgArc<S>) -> Arc<S> {
        debug_assert!(!arc.from.x.is_nan());
        debug_assert!(!arc.from.y.is_nan());
        debug_assert!(!arc.to.x.is_nan());
        debug_assert!(!arc.to.y.is_nan());
        debug_assert!(!arc.radii.x.is_nan());
        debug_assert!(!arc.radii.y.is_nan());
        debug_assert!(!arc.x_rotation.get().is_nan());

        let rx = arc.radii.x;
        let ry = arc.radii.y;

        assert_ne!(arc.from, arc.to);
        assert_ne!(rx, S::ZERO);
        assert_ne!(ry, S::ZERO);

        let xr = arc.x_rotation.get() % (S::TWO * S::PI());
        let cos_phi = Float::cos(xr);
        let sin_phi = Float::sin(xr);
        let hd_x = (arc.from.x - arc.to.x) / S::TWO;
        let hd_y = (arc.from.y - arc.to.y) / S::TWO;
        let hs_x = (arc.from.x + arc.to.x) / S::TWO;
        let hs_y = (arc.from.y + arc.to.y) / S::TWO;
        // F6.5.1
        let p = Point::new(
            cos_phi * hd_x + sin_phi * hd_y,
            -sin_phi * hd_x + cos_phi * hd_y,
        );

        // TODO: sanitize radii

        let rxry = rx * ry;
        let rxpy = rx * p.y;
        let rypx = ry * p.x;
        let sum_of_sq = rxpy * rxpy + rypx * rypx;

        debug_assert_ne!(sum_of_sq, S::ZERO);

        let sign_coe = if arc.flags.large_arc == arc.flags.sweep {-S::ONE } else { S::ONE };
        let coe = sign_coe * S::sqrt(S::abs((rxry * rxry - sum_of_sq) / sum_of_sq));

        let transformed_cx = coe * rxpy / ry;
        let transformed_cy = -coe * rypx / rx;

        // F6.5.3
        let center = point(
            cos_phi * transformed_cx - sin_phi * transformed_cy + hs_x,
            sin_phi * transformed_cx + cos_phi * transformed_cy + hs_y
        );

        let a = vector(
            (p.x - transformed_cx) / rx,
            (p.y - transformed_cy) / ry,
        );
        // TODO
        let b = -vector(
            (-p.x - transformed_cx) / rx,
            (-p.y - transformed_cy) / ry,
        );

        let start_angle = Angle::radians(directed_angle(vector(S::ONE, S::ZERO), a));

        let sign_delta = if arc.flags.sweep { S::ONE } else { -S::ONE };
        let sweep_angle = Angle::radians(sign_delta * (S::abs(directed_angle(a, b)) % (S::TWO * S::PI())));

        Arc {
            center: center,
            radii: arc.radii,
            start_angle: start_angle,
            sweep_angle: sweep_angle,
            x_rotation: arc.x_rotation
        }
    }

    pub fn to_svg_arc(&self) -> SvgArc<S> {
        let from = self.sample(S::ZERO);
        let to = self.sample(S::ONE);
        let flags = ArcFlags {
            sweep: S::abs(self.sweep_angle.get()) >= S::PI(),
            large_arc: self.sweep_angle.get() >= S::ZERO,
        };
        SvgArc {
            from,
            to,
            radii: self.radii,
            x_rotation: self.x_rotation,
            flags,
        }
    }

    #[inline]
    pub fn for_each_quadratic_bezier<F>(&self, cb: &mut F)
    where
        F: FnMut(&QuadraticBezierSegment<S>)
    {
        arc_to_to_quadratic_beziers(self, cb);
    }

    /// Sample the curve at t (expecting t between 0 and 1).
    #[inline]
    pub fn sample(&self, t: S) -> Point<S> {
        let angle = self.get_angle(t);
        self.center + sample_ellipse(self.radii, self.x_rotation, angle).to_vector()
    }

    #[inline]
    pub fn x(&self, t: S) -> S { self.sample(t).x }

    #[inline]
    pub fn y(&self, t: S) -> S { self.sample(t).y }

    /// Sample the curve's tangent at t (expecting t between 0 and 1).
    #[inline]
    pub fn sample_tangent(&self, t: S) -> Vector<S> {
        self.tangent_at_angle(self.get_angle(t))
    }

    /// Sample the curve's angle at t (expecting t between 0 and 1).
    #[inline]
    pub fn get_angle(&self, t: S) -> Angle<S> {
        self.start_angle + Angle::radians(self.sweep_angle.get() * t)
    }

    #[inline]
    pub fn end_angle(&self) -> Angle<S> {
        self.start_angle + self.sweep_angle
    }

    #[inline]
    pub fn from(&self) -> Point<S> {
        self.sample(S::ZERO)
    }

    #[inline]
    pub fn to(&self) -> Point<S> {
        self.sample(S::ONE)
    }

    /// Return the sub-curve inside a given range of t.
    ///
    /// This is equivalent splitting at the range's end points.
    pub fn split_range(&self, t_range: Range<S>) -> Self {
        let angle_1 = Angle::radians(self.sweep_angle.get() * t_range.start);
        let angle_2 = Angle::radians(self.sweep_angle.get() * t_range.end);

        Arc {
            center: self.center,
            radii: self.radii,
            start_angle: self.start_angle + angle_1,
            sweep_angle: angle_2 - angle_1,
            x_rotation: self.x_rotation,
        }
    }

    /// Split this curve into two sub-curves.
    pub fn split(&self, t: S) -> (Arc<S>, Arc<S>) {
        let split_angle = Angle::radians(self.sweep_angle.get() * t);
        (
            Arc {
                center: self.center,
                radii: self.radii,
                start_angle: self.start_angle,
                sweep_angle: split_angle,
                x_rotation: self.x_rotation,
            },
            Arc {
                center: self.center,
                radii: self.radii,
                start_angle: self.start_angle + split_angle,
                sweep_angle: self.sweep_angle - split_angle,
                x_rotation: self.x_rotation,
            },
        )
    }

    /// Return the curve before the split point.
    pub fn before_split(&self, t: S) -> Arc<S> {
        let split_angle = Angle::radians(self.sweep_angle.get() * t);
        Arc {
            center: self.center,
            radii: self.radii,
            start_angle: self.start_angle,
            sweep_angle: split_angle,
            x_rotation: self.x_rotation,
        }
    }

    /// Return the curve after the split point.
    pub fn after_split(&self, t: S) -> Arc<S> {
        let split_angle = Angle::radians(self.sweep_angle.get() * t);
        Arc {
            center: self.center,
            radii: self.radii,
            start_angle: self.start_angle + split_angle,
            sweep_angle: self.sweep_angle - split_angle,
            x_rotation: self.x_rotation,
        }
    }

    /// Swap the direction of the segment.
    pub fn flip(&self) -> Self {
        let mut arc = *self;
        arc.start_angle = arc.start_angle + self.sweep_angle;
        arc.sweep_angle = -self.sweep_angle;

        arc
    }

    /// Iterates through the curve invoking a callback at each point.
    pub fn for_each_flattened<F: FnMut(Point<S>)>(&self, tolerance: S, call_back: &mut F) {
        <Self as FlattenedForEach>::for_each_flattened(self, tolerance, call_back);
    }

    /// Iterates through the curve invoking a callback at each point.
    pub fn flattening_step(&self, tolerance: S) -> S {
        // Here we make the approximation that for small tolerance values we consider
        // the radius to be constant over each approximated segment.
        let r = (self.from() - self.center).length();
        let a = S::TWO * tolerance * r - tolerance * tolerance;
        S::acos((a * a) / r)
    }

    /// Returns the flattened representation of the curve as an iterator, starting *after* the
    /// current point.
    pub fn flattened(&self, tolerance: S) -> Flattened<S> {
        Flattened::new(*self, tolerance)
    }

    /// Returns a conservative rectangle that contains the curve.
    pub fn bounding_rect(&self) -> Rect<S> {
        Transform2D::create_rotation(self.x_rotation).transform_rect(
            &Rect::new(
                self.center - self.radii,
                self.radii.to_size() * S::TWO
            )
        )
    }

    pub fn bounding_range_x(&self) -> (S, S) {
        let r = self.bounding_rect();
        (r.min_x(), r.max_x())
    }

    pub fn bounding_range_y(&self) -> (S, S) {
        let r = self.bounding_rect();
        (r.min_y(), r.max_y())
    }

    pub fn approximate_length(&self, tolerance: S) -> S {
        segment::approximate_length_from_flattening(self, tolerance)
    }

    #[inline]
    fn tangent_at_angle(&self, angle: Angle<S>) -> Vector<S> {
        let a = angle.get();
        Rotation2D::new(self.x_rotation).transform_vector(
            &vector(-self.radii.x * Float::sin(a), self.radii.y * Float::cos(a))
        )
    }
}

impl<S: Scalar> Into<Arc<S>> for SvgArc<S> {
    fn into(self) -> Arc<S> { self.to_arc() }
}

impl<S: Scalar> SvgArc<S> {
    pub fn to_arc(&self) -> Arc<S> { Arc::from_svg_arc(self) }

    pub fn for_each_quadratic_bezier<F>(&self, cb: &mut F)
    where
        F: FnMut(&QuadraticBezierSegment<S>)
    {
        Arc::from_svg_arc(self).for_each_quadratic_bezier(cb);
    }
}

/// Flag parameters for arcs as described by the SVG specification.
#[derive(Copy, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serialization", derive(Serialize, Deserialize))]
pub struct ArcFlags {
    pub large_arc: bool,
    pub sweep: bool,
}

impl Default for ArcFlags {
    fn default() -> Self {
        ArcFlags {
            large_arc: false,
            sweep: false,
        }
    }
}

fn arc_to_to_quadratic_beziers<S, F>(
    arc: &Arc<S>,
    callback: &mut F,
)
where
    S: Scalar,
    F: FnMut(&QuadraticBezierSegment<S>)
{
    let sign = arc.sweep_angle.get().signum();
    let sweep_angle = S::abs(arc.sweep_angle.get()).min(S::PI() * S::TWO);

    let n_steps = S::ceil(sweep_angle / S::FRAC_PI_4());
    let step = Angle::radians(sweep_angle / n_steps * sign);

    for i in 0..cast::<S, i32>(n_steps).unwrap() {
        let a1 = arc.start_angle + step * cast(i).unwrap();
        let a2 = arc.start_angle + step * cast(i+1).unwrap();

        let v1 = sample_ellipse(arc.radii, arc.x_rotation, a1).to_vector();
        let v2 = sample_ellipse(arc.radii, arc.x_rotation, a2).to_vector();
        let from = arc.center + v1;
        let to = arc.center + v2;
        let l1 = Line { point: from, vector: arc.tangent_at_angle(a1) };
        let l2 = Line { point: to, vector: arc.tangent_at_angle(a2) };
        let ctrl = l2.intersection(&l1).unwrap();

        callback(&QuadraticBezierSegment { from , ctrl, to });
    }
}

fn sample_ellipse<S: Scalar>(radii: Vector<S>, x_rotation: Angle<S>, angle: Angle<S>) -> Point<S> {
    Rotation2D::new(x_rotation).transform_point(
        &point(radii.x * Float::cos(angle.get()), radii.y * Float::sin(angle.get()))
    )
}

impl<S: Scalar> Segment for Arc<S> {
    type Scalar = S;
    fn from(&self) -> Point<S> { self.from() }
    fn to(&self) -> Point<S> { self.to() }
    fn sample(&self, t: S) -> Point<S> { self.sample(t) }
    fn x(&self, t: S) -> S { self.x(t) }
    fn y(&self, t: S) -> S { self.y(t) }
    fn derivative(&self, t: S) -> Vector<S> { self.sample_tangent(t) }
    fn split_range(&self, t_range: Range<S>) -> Self { self.split_range(t_range) }
    fn split(&self, t: S) -> (Self, Self) { self.split(t) }
    fn before_split(&self, t: S) -> Self { self.before_split(t) }
    fn after_split(&self, t: S) -> Self { self.after_split(t) }
    fn flip(&self) -> Self { self.flip() }
    fn approximate_length(&self, tolerance: S) -> S {
        self.approximate_length(tolerance)
    }
}

impl<S: Scalar> BoundingRect for Arc<S> {
    type Scalar = S;
    fn bounding_rect(&self) -> Rect<S> { self.bounding_rect() }
    fn fast_bounding_rect(&self) -> Rect<S> { self.bounding_rect() }
    fn bounding_range_x(&self) -> (S, S) { self.bounding_range_x() }
    fn bounding_range_y(&self) -> (S, S) { self.bounding_range_y() }
    fn fast_bounding_range_x(&self) -> (S, S) { self.bounding_range_x() }
    fn fast_bounding_range_y(&self) -> (S, S) { self.bounding_range_y() }
}

impl<S: Scalar> FlatteningStep for Arc<S> {
    fn flattening_step(&self, tolerance: S) -> S {
        self.flattening_step(tolerance)
    }
}

#[test]
fn test_to_quadratics() {
    use euclid::approxeq::ApproxEq;

    fn do_test(arc: &Arc<f32>, expexted_count: u32) {
        let mut prev = arc.from();
        let mut count = 0;
        arc.for_each_quadratic_bezier(&mut|c| {
            assert!(c.from.approx_eq(&prev));
            prev = c.to;
            count += 1;
        });
        let last = arc.to();
        assert!(prev.approx_eq(&last));
        assert_eq!(count, expexted_count);
    }

    do_test(
        &Arc {
            center: point(2.0, 3.0),
            radii: vector(10.0, 3.0),
            start_angle: Angle::radians(0.1),
            sweep_angle: Angle::radians(3.0),
            x_rotation: Angle::radians(0.5),
        },
        4
    );

    do_test(
        &Arc {
            center: point(4.0, 5.0),
            radii: vector(3.0, 5.0),
            start_angle: Angle::radians(2.0),
            sweep_angle: Angle::radians(-3.0),
            x_rotation: Angle::radians(1.3),
        },
        4
    );

    do_test(
        &Arc {
            center: point(0.0, 0.0),
            radii: vector(100.0, 0.01),
            start_angle: Angle::radians(-1.0),
            sweep_angle: Angle::radians(0.1),
            x_rotation: Angle::radians(0.3),
        },
        1
    );

    do_test(
        &Arc {
            center: point(0.0, 0.0),
            radii: vector(1.0, 1.0),
            start_angle: Angle::radians(3.0),
            sweep_angle: Angle::radians(-0.1),
            x_rotation: Angle::radians(-0.3),
        },
        1
    );
}
