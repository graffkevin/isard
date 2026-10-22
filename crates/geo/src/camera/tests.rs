//! Tests of the camera: projections, picking, interactions and the frames of reference.

use super::*;
use crate::geodesy;
use glam::{DMat4, DVec4};

fn camera() -> Camera {
    let mut c = Camera::new(2.424_573, 48.845_726, 14.0);

    c.width = 1200.0;
    c.height = 800.0;

    c
}

fn globe(zoom: f64) -> Camera {
    let mut c = camera();

    c.globe = true;
    c.zoom = zoom;

    c
}

#[test]
fn a_street_level_eye_stays_where_it_stands() {
    let mut c = Camera::new(2.3348, 48.8506, 20.0);

    c.width = 1000.0;
    c.height = 650.0;
    c.first_person = Some(FirstPerson {
        height: 2.5,
        fov: 75f64.to_radians(),
    });
    c.pitch = 120.0;

    let center = c.center;

    // Finer relief arriving under it: the eye rises with the ground, neither slides nor zooms nor tilts
    c.settle_elevation(80.0);
    assert_eq!((c.center, c.zoom, c.pitch, c.elevation), (center, 20.0, 120.0, 80.0));
    c.clamp();
    assert!((c.pitch - 120.0).abs() < 1e-12, "{}", c.pitch);

    // Looking up: the eye 2.5 m above the ground, under the far plane's sky
    let view = c.view();
    let k = view.vertical;

    assert!((view.eye.z - (80.0 + 2.5) * k).abs() < 1e-6 * k, "{}", view.eye.z / k);
}

#[test]
fn mercator_round_trip() {
    let m = mercator(2.424_573, 48.845_726);
    let back = lon_lat(m);

    assert!((back.x - 2.424_573).abs() < 1e-9 && (back.y - 48.845_726).abs() < 1e-9);
}

#[test]
fn screen_center_is_camera_center() {
    for (pitch, bearing) in [(0.0, 0.0), (60.0, 0.0), (45.0, -17.0), (70.0, 120.0)] {
        for mut c in [camera(), globe(3.0)] {
            c.pitch = pitch;
            c.bearing = bearing;

            let g = c.ground(&c.view(), 600.0, 400.0).unwrap();

            assert!(
                (g - c.center).length() < 1e-12,
                "pitch {pitch} bearing {bearing} globe {}",
                c.globe
            );
        }
    }
}

#[test]
fn north_is_up_and_east_is_right() {
    let c = camera();
    let v = c.view();
    let top = c.ground(&v, 600.0, 0.0).unwrap();
    let right = c.ground(&v, 1200.0, 400.0).unwrap();

    assert!(top.y < c.center.y && (top.x - c.center.x).abs() < 1e-12);
    assert!(right.x > c.center.x && (right.y - c.center.y).abs() < 1e-12);
}

#[test]
fn bearing_turns_the_map_clockwise() {
    // Bearing 90: the east is at the top of the screen
    let mut c = camera();

    c.bearing = 90.0;

    let top = c.ground(&c.view(), 600.0, 0.0).unwrap();

    assert!(top.x > c.center.x && (top.y - c.center.y).abs() < 1e-12);
}

#[test]
fn pitch_pushes_the_top_of_the_screen_further() {
    let mut c = camera();
    let flat = c.center.y - c.ground(&c.view(), 600.0, 0.0).unwrap().y;

    c.pitch = 60.0;

    let tilted = c.center.y - c.ground(&c.view(), 600.0, 0.0).unwrap().y;

    assert!(tilted > 2.0 * flat);
}

#[test]
fn zoom_keeps_the_point_under_the_cursor() {
    for mut c in [camera(), globe(3.0)] {
        c.pitch = 30.0;

        let at = DVec2::new(800.0, 300.0);
        let before = c.ground(&c.view(), at.x, at.y).unwrap();

        c.zoom_at(0.3, at);

        let after = c.ground(&c.view(), at.x, at.y).unwrap();

        assert!(
            (before - after).length() < 1e-4,
            "globe {}: {}",
            c.globe,
            (before - after).length()
        );
    }
}

#[test]
fn pan_moves_the_grabbed_point() {
    let mut c = camera();

    c.pitch = 45.0;

    let (from, to) = (DVec2::new(300.0, 500.0), DVec2::new(700.0, 350.0));
    let grabbed = c.ground(&c.view(), from.x, from.y).unwrap();

    c.pan(from, to);

    let now = c.ground(&c.view(), to.x, to.y).unwrap();

    assert!((grabbed - now).length() < 1e-11);
}

#[test]
fn sphere_touches_the_plane_at_the_center() {
    let v = globe(3.0).view();

    assert!(v.sphere(v.center).length() < 1e-6);

    // One pixel east on the plane is one pixel east on the sphere
    let m = v.center + DVec2::new(1.0 / v.world_size, 0.0);
    let s = v.sphere(m);

    assert!((s.x - 1.0).abs() < 1e-3 && s.y.abs() < 1e-3, "{s}");
    assert!((v.sphere_to_mercator(s) - m).length() < 1e-12);
}

#[test]
fn ground_at_an_altitude() {
    // A point 900 m up seen from a tilted camera standing over a 1000 m relief: back to it through its screen point
    for globe_on in [false, true] {
        let mut c = camera();

        c.globe = globe_on;
        c.zoom = if globe_on { 4.0 } else { 14.0 };
        c.pitch = 55.0;
        c.elevation = 1000.0;

        let view = c.view();
        let m = c.center + DVec2::new(0.6, -0.4) / view.world_size * 300.0;
        let base = view.position(m, 0.0);
        let up = if view.globe.factor >= 0.5 {
            (base - view.sphere_center()).normalize()
        } else {
            DVec3::Z
        };
        let p = base + up * 900.0 * view.vertical;
        let clip = view.view_proj * p.extend(1.0);
        let (x, y) = (
            f64::midpoint(clip.x / clip.w, 1.0) * c.width,
            (1.0 - clip.y / clip.w) / 2.0 * c.height,
        );
        let found = c.ground_at(&view, x, y, 900.0).expect("on the ground");

        assert!((found - m).length() * view.world_size < 0.01, "{globe_on}: {found} {m}");
        assert!((Camera::altitude_of(&view, p) - 900.0).abs() < 0.01);

        // At the center's altitude instead, the ray meets the ground elsewhere (100 m off: visible at zoom 14)
        if !globe_on {
            let wrong = c.ground(&view, x, y).expect("on the ground");

            assert!((wrong - m).length() * view.world_size > 1.0);
        }
    }
}

#[test]
fn grabbed_relief_stays_under_the_cursor() {
    // A slope rising 2000 m per 0.001 of Mercator y towards the north, the camera tilted over it
    let slope = |m: DVec2| Some(1000.0 + (0.3384 - m.y) * 2e6);
    let mut c = camera();

    c.pitch = 65.0;
    c.zoom = 13.5;
    c.elevation = slope(c.center).unwrap();

    let (from, to) = (DVec2::new(900.0, 250.0), DVec2::new(700.0, 330.0));
    // The relief under `from`: search the altitude where the ray meets the slope
    let view = c.view();
    let mut altitude = c.elevation;

    for _ in 0..50 {
        altitude = slope(c.ground_at(&view, from.x, from.y, altitude).unwrap()).unwrap();
    }

    let grabbed = c.ground_at(&view, from.x, from.y, altitude).unwrap();

    for k in 1..=10 {
        let a = from + (to - from) * f64::from(k - 1) / 10.0;
        let b = from + (to - from) * f64::from(k) / 10.0;

        c.pan_at(a, b, altitude);
        // Each frame brings the orbited point down to the ground under the new center, the eye staying
        c.settle_elevation(slope(c.center).unwrap());
    }

    let under = c.ground_at(&c.view(), to.x, to.y, altitude).unwrap();
    let error = (under - grabbed).length() * c.view().world_size;

    assert!(error < 0.05, "{error} px");
    // The settled center moves too: the next frames converge on the ground, still without moving the grabbed point
    for _ in 0..20 {
        c.settle_elevation(slope(c.center).unwrap());
    }
    assert!((c.elevation - slope(c.center).unwrap()).abs() < 0.01);

    let under = c.ground_at(&c.view(), to.x, to.y, altitude).unwrap();

    assert!((under - grabbed).length() * c.view().world_size < 0.05);
}

#[test]
fn finer_relief_does_not_move_the_view() {
    let mut c = camera();

    c.pitch = 60.0;
    c.bearing = 30.0;
    c.zoom = 14.0;
    c.elevation = 800.0;

    let before = c.view();
    // A ground point 1000 m up, off the center: where it shows before and after the center's altitude changes
    let p = before.position(c.center + DVec2::new(150.0, -90.0) / before.world_size, 0.0)
        + DVec3::Z * 1000.0 * before.vertical;
    let screen = |c: &Camera, view: &View| {
        let m = c.center + DVec2::new(p.x, p.y) / before.world_size + before.center - c.center;
        let q = view.position(m, 0.0) + DVec3::Z * 1000.0 * view.vertical;
        let clip = view.view_proj * q.extend(1.0);

        DVec2::new(clip.x / clip.w, clip.y / clip.w)
    };
    let a = screen(&c, &before);

    c.settle_elevation(950.0);

    let after = c.view();
    let b = screen(&c, &after);

    assert!((a - b).length() * c.width < 0.05, "{a} {b}");
    assert!((c.elevation - 950.0).abs() < 1e-9);
}

#[test]
fn picking_meets_the_blended_globe_as_drawn() {
    // Halfway between the sphere and the plane, looking down or tilted: a screen point, the ground found under
    // it, and where that ground is drawn again; the altitude of a point above it found back
    for (zoom, pitch) in [(6.0, 0.0), (7.5, 55.0), (8.0, 64.0), (9.0, 70.0)] {
        let mut c = globe(zoom);

        c.max_pitch = PITCH_LIMIT;
        c.width = 1600.0;
        c.height = 900.0;
        c.center = mercator(2.0, 45.0);
        c.pitch = pitch;
        c.bearing = 30.0;

        let view = c.view();
        let f = view.globe.factor;

        assert!(f > 0.05 && f < 0.95, "zoom {zoom} pitch {pitch}: {f}");

        for altitude in [0.0, 2000.0] {
            let h = altitude * view.vertical;

            for (x, y) in [
                (800.0, 450.0),
                (20.0, 880.0),
                (1580.0, 20.0),
                (800.0, 60.0),
                (300.0, 300.0),
            ] {
                let Some(m) = c.ground_at(&view, x, y, altitude) else {
                    // Over the horizon: the ray stays above the surface
                    let (a, b) = c.ray(&view, x, y);
                    let lowest = (0..=1000)
                        .map(|k| Camera::altitude_of(&view, a + (b - a) * f64::from(k) / 1000.0))
                        .fold(f64::INFINITY, f64::min);

                    assert!(lowest > altitude, "zoom {zoom} pitch {pitch} at ({x}, {y}): missed");
                    continue;
                };
                let p = view.surface(m, h);
                let clip = view.view_proj * p.extend(1.0);
                let screen = DVec2::new(
                    (clip.x / clip.w + 1.0) * c.width / 2.0,
                    (1.0 - clip.y / clip.w) * c.height / 2.0,
                );
                let error = (screen - DVec2::new(x, y)).length();

                assert!(error < 0.05, "zoom {zoom} pitch {pitch} at ({x}, {y}): {error} px");

                let found = Camera::altitude_of(&view, view.surface(m, h + 500.0 * view.vertical));

                // Within a hundredth of a pixel
                assert!((found - altitude - 500.0).abs() * view.vertical < 0.01, "{found} m");
            }
        }
    }
}

#[test]
fn finer_relief_does_not_move_a_tilted_globe() {
    // Over the Alps at zoom 8.5, pitch 70: still partly a sphere, the relief tens of pixels tall
    for (zoom, pitch) in [(8.5, 70.0), (6.0, 50.0), (4.0, 0.0)] {
        let mut c = globe(zoom);

        c.max_pitch = PITCH_LIMIT;
        c.center = mercator(7.0, 46.0);
        c.pitch = pitch;
        c.bearing = 20.0;
        c.elevation = 1500.0;
        assert!(c.view().globe.factor > 0.0);

        // Ground points on the datum, near the center and towards the top of the view
        let points: Vec<DVec2> = [(0.0, 0.0), (120.0, -60.0), (-80.0, -300.0)]
            .iter()
            .map(|&(x, y)| c.center + DVec2::new(x, y) / c.view().world_size)
            .collect();
        let screen = |c: &Camera| -> Vec<DVec2> {
            let view = c.view();

            points
                .iter()
                .map(|&m| {
                    let clip = view.view_proj * view.position(m, 0.0).extend(1.0);

                    DVec2::new(clip.x / clip.w, clip.y / clip.w) * c.width
                })
                .collect()
        };
        let a = screen(&c);

        c.settle_elevation(1800.0);

        let b = screen(&c);
        let moved = a.iter().zip(&b).map(|(a, b)| (*a - *b).length()).fold(0.0, f64::max);

        assert!(moved < 0.5, "zoom {zoom} pitch {pitch}: {moved} px");
        assert!((c.elevation - 1800.0).abs() < 1e-9);
    }
}

#[test]
fn far_side_of_the_globe_is_hidden() {
    let v = globe(1.0).view();
    let antipode = DVec2::new((v.center.x + 0.5).rem_euclid(1.0), 1.0 - v.center.y);

    assert!(v.faces_camera(v.sphere(v.center)));
    assert!(!v.faces_camera(v.sphere(antipode)));

    // Spheres: the antipode hidden, the center and a huge sphere around the antipode not
    let k = v.globe.radius / v.body.radius;

    assert!(v.behind_horizon(v.sphere(antipode), 1000.0 * k));
    assert!(!v.behind_horizon(v.sphere(v.center), 1000.0 * k));
    assert!(!v.behind_horizon(v.sphere(antipode), 8e6 * k));

    // The ECEF position without the matrix is the matrix's origin
    let p = geodesy::to_ecef(3.06, 50.63, 120.0);

    assert!((v.ecef(p) - v.ecef_matrix(p).w_axis.truncate()).length() < 1e-9);
}

#[test]
fn deep_ground_of_the_body_is_not_hidden() {
    // A point 5 km under the sphere, where its horizon passes: inside the globe shrunk by 1 km, hidden; Mars's
    // ground goes 8 km deep (Hellas), its globe is shrunk by as much and the point shows
    let mut c = globe(1.5);

    c.body = Body::MARS;

    let deep = |c: &Camera| {
        let v = c.view();
        let (center, r) = (v.sphere_center(), v.globe.radius);
        let to_eye = v.eye - center;
        let d = to_eye.length();
        let u = to_eye / d;
        let w = u.cross(DVec3::X).normalize();
        let a = (r / d).acos();
        let p = center + (u * a.cos() + w * a.sin()) * r * (1.0 - 5e3 / v.body.radius);

        v.behind_horizon(p, 1e-6)
    };

    assert!(!deep(&c));
    c.body.relief.0 = -200.0;
    assert!(deep(&c));
}

#[test]
fn globe_blends_into_the_plane() {
    assert!((globe(4.0).view().globe.factor - 1.0).abs() < 1e-12);
    assert!(globe(7.5).view().globe.factor.abs() < 1e-12);
    assert!(camera().view().globe.factor.abs() < 1e-12);
}

#[test]
fn a_tilted_globe_stays_a_sphere_longer() {
    let tilted = |zoom: f64, pitch: f64| {
        let mut c = globe(zoom);

        c.max_pitch = PITCH_LIMIT;
        c.pitch = pitch;

        c.view().globe.factor
    };

    // Looking down, as before; Olympus Mons from the south at zoom 6.3, pitch 66: a sphere to its horizon
    assert!((tilted(6.0, 0.0) - 0.5).abs() < 1e-12);
    assert!((tilted(6.3, 66.0) - 1.0).abs() < 1e-12);
    // Never past zoom 10, whatever the tilt
    assert!(tilted(10.0, 85.0).abs() < 1e-12);
    assert!(tilted(9.5, 85.0) > 0.0);

    // The blend grows with the tilt
    let factors: Vec<f64> = (0..=85).step_by(5).map(|p| tilted(8.0, f64::from(p))).collect();

    assert!(factors.windows(2).all(|w| w[0] <= w[1]), "{factors:?}");
    assert!(tilted(7.0, 30.0) < tilted(7.0, 50.0));

    // In fine steps, through the top edge reaching the horizon: no jump, no NaN
    let fine: Vec<f64> = (0..=1700).map(|p| tilted(8.0, f64::from(p) * 0.05)).collect();

    assert!(fine.iter().all(|f| f.is_finite()));
    assert!(fine.windows(2).all(|w| w[0] <= w[1] && w[1] - w[0] < 0.05), "{fine:?}");
}

#[test]
fn dragging_the_globe_keeps_its_size() {
    // From France towards the north pole and back: the globe turns, its radius on screen stays the same
    let mut c = globe(2.5);
    let radius = c.view().globe.radius;

    for _ in 0..10 {
        c.pan(DVec2::new(600.0, 300.0), DVec2::new(600.0, 420.0));
    }
    assert!(lon_lat(c.center).y > 60.0, "{}", lon_lat(c.center).y);
    assert!((c.view().globe.radius / radius - 1.0).abs() < 1e-9);
    for _ in 0..20 {
        c.pan(DVec2::new(600.0, 420.0), DVec2::new(600.0, 300.0));
    }
    assert!(lon_lat(c.center).y < 30.0, "{}", lon_lat(c.center).y);
    assert!((c.view().globe.radius / radius - 1.0).abs() < 1e-9);
}

#[test]
fn dragging_the_globe_follows_the_cursor() {
    let mut c = globe(2.5);
    let (from, to) = (DVec2::new(600.0, 300.0), DVec2::new(640.0, 360.0));
    let grabbed = lon_lat(c.ground(&c.view(), from.x, from.y).unwrap());

    c.pan(from, to);

    let now = lon_lat(c.ground(&c.view(), to.x, to.y).unwrap());

    assert!((grabbed - now).length() < 1e-6, "{grabbed} → {now}");
}

#[test]
fn ecef_points_land_where_the_basemap_puts_them() {
    // A ground point 300 m east of the center, through the ECEF map, lands where the Mercator plane has it
    for body in [Body::EARTH, Body::MOON, Body::MARS] {
        for globe_zoom in [None, Some(3.0), Some(6.0)] {
            let mut c = camera();

            c.body = body;
            if let Some(z) = globe_zoom {
                c.globe = true;
                c.zoom = z;
            }

            let v = c.view();
            let center = lon_lat(c.center);
            let anchor = body.to_ecef(center.x, center.y, 0.0);

            assert!(v.ecef(anchor).length() < 1e-6, "anchor at the center");

            // Degrees of longitude for 300 m along the parallel
            let lon = center.x + 300.0 / (body.circumference() / 360.0 * center.y.to_radians().cos());
            let east = body.to_ecef(lon, center.y, 0.0);
            let through_matrix = v.ecef_matrix(anchor).transform_point3(east - anchor);
            let direct = v.position(mercator(lon, center.y), 0.0);
            let scale = v.pixels_per_metre(center.y) * 300.0;

            assert!(
                (through_matrix - direct).length() < 0.02 * scale + 0.01,
                "{body:?} {globe_zoom:?}: {through_matrix} vs {direct}"
            );

            // Up is up, scaled like the ground
            let high = v.ecef(body.to_ecef(center.x, center.y, 100.0));

            assert!(
                (high.z - 100.0 * v.pixels_per_metre(center.y)).abs() < 1e-3 * high.z.abs().max(1.0),
                "{body:?}: {high}"
            );
        }
    }
}

#[test]
fn enu_frames_on_the_plane() {
    // At the center, on the ground: the origin of the local frame, east right, north up the screen (y south), the
    // scale of Web Mercator at that latitude
    let v = camera().view();
    let (lon, lat) = (2.424_573, 48.845_726);
    let m = v.enu_matrix(DVec3::new(lon, lat, 0.0));
    let k = 512.0 * 2f64.powi(14) / (2.0 * PI * 6_378_137.0 * lat.to_radians().cos());
    let expected = DMat4::from_cols(
        DVec4::new(k, 0.0, 0.0, 0.0),
        DVec4::new(0.0, -k, 0.0, 0.0),
        DVec4::new(0.0, 0.0, k, 0.0),
        DVec4::W,
    );

    assert!(m.abs_diff_eq(expected, 1e-9), "{m}");
    assert!((k - 0.317_9).abs() < 1e-3, "{k}");

    // 1 km north and 50 m up: where the Mercator plane puts that point, within the change of scale over 1 km
    let north = lat + 1000.0 / 111_210.0;
    let through = v
        .enu_matrix(DVec3::new(lon, lat, 50.0))
        .transform_point3(DVec3::new(0.0, 1000.0, 0.0));
    let direct = v.position(mercator(lon, north), 0.0) + DVec3::Z * 50.0 * k;

    assert!(
        (through - direct).length() < 0.002 * 1000.0 * k,
        "{through} vs {direct}"
    );

    // A point of another copy of the world is drawn on the one nearest to the center
    let wrapped = v.enu_matrix(DVec3::new(lon + 360.0, lat, 0.0));

    assert!(wrapped.w_axis.truncate().length() < 1e-6);
}

#[test]
fn enu_frames_on_the_globe() {
    let v = globe(3.0).view();
    let k = v.globe.radius / v.body.radius;

    for (lon, lat) in [(2.424_573, 48.845_726), (30.0, 10.0), (-20.0, 60.0)] {
        let m = v.enu_matrix(DVec3::new(lon, lat, 500.0));
        let (e, n, u) = (m.x_axis.truncate(), m.y_axis.truncate(), m.z_axis.truncate());

        // A rigid frame scaled to the sphere: orthogonal axes of k pixels per metre, east × north = up once the local
        // frame's y (south) is flipped back, as the view does
        for axis in [e, n, u] {
            assert!((axis.length() / k - 1.0).abs() < 1e-9);
        }
        assert!(e.dot(n).abs() < 1e-9 * k * k && e.dot(u).abs() < 1e-9 * k * k);
        assert!((e.cross(n) / k + u).length() < 1e-9 * k);

        // Up is the sphere's radius there, the origin 500 m above the ground point
        let ground = v.sphere(mercator(lon, lat));
        let radial = (ground - v.sphere_center()).normalize();

        assert!((u / k - radial).length() < 1e-9, "{lon} {lat}");
        assert!((m.w_axis.truncate() - (ground + radial * 500.0 * k)).length() < 1e-6);

        // North points to the pole: towards +z of the body
        let pole = v.sphere(mercator(lon, 85.0));

        assert!(n.dot(pole - ground) > 0.0);
        // The origin is `geodetic`'s
        assert!((m.w_axis.truncate() - v.geodetic(DVec3::new(lon, lat, 500.0))).length() < 1e-6);
    }
}

#[test]
fn enu_frames_stay_exact_in_f32() {
    // At zoom 22, the model-view-projection of a point 10 m east of the center, made in f64 and handed over in
    // f32 (as the custom layers get it), puts the point where f64 does, to a hundredth of a pixel
    let mut c = camera();

    c.zoom = 22.0;
    c.pitch = 60.0;

    let v = c.view();
    let at = DVec3::new(2.424_573 + 0.000_1, 48.845_726, 2.0);
    let mvp = v.view_proj * v.enu_matrix(at);
    let f32_mvp = mvp.as_mat4();
    let local = DVec3::new(3.0, -2.0, 1.5);
    let exact = mvp.project_point3(local);
    let rounded = f32_mvp.project_point3(local.as_vec3()).as_dvec3();
    let pixels = (exact - rounded).truncate() * DVec2::new(c.width, c.height) / 2.0;

    assert!(pixels.length() < 0.01, "{pixels}");
}

#[test]
fn metres_follow_the_body() {
    // A zoom level is the same Web Mercator pyramid on any body: its pixels hold fewer metres of a smaller one
    for globe_on in [false, true] {
        let mut earth = camera();

        earth.globe = globe_on;
        earth.zoom = if globe_on { 3.0 } else { 14.0 };

        let mut moon = earth.clone();

        moon.body = Body::MOON;

        let (e, m) = (earth.view(), moon.view());
        let lat = lon_lat(earth.center).y;
        // The Earth's scale as before: Web Mercator's on the plane, the sphere's on the globe
        let expected = if globe_on {
            e.globe.radius / 6_378_137.0
        } else {
            e.world_size / (2.0 * PI * 6_378_137.0 * lat.to_radians().cos())
        };

        assert!((e.pixels_per_metre(lat) / expected - 1.0).abs() < 1e-12);
        assert!((e.vertical / expected - 1.0).abs() < 1e-12);

        // The Moon's, its radius over the Earth's larger; the globe itself the same size on screen
        let ratio = 6_378_137.0 / 1_737_400.0;

        assert!((m.pixels_per_metre(lat) / e.pixels_per_metre(lat) / ratio - 1.0).abs() < 1e-12);
        assert!((m.vertical / e.vertical / ratio - 1.0).abs() < 1e-12);
        assert!((m.globe.radius - e.globe.radius).abs() < 1e-9);
        assert_eq!(m.view_proj, e.view_proj);
    }

    // A camera over the relief stands the same number of metres higher: more pixels on the Moon
    let mut c = camera();

    c.elevation = 1000.0;

    let earth = c.view().lift;

    c.body = Body::MOON;
    assert!((c.view().lift / earth - 6_378_137.0 / 1_737_400.0).abs() < 1e-9);
}

#[test]
fn whole_globe_fits_at_zoom_zero() {
    // Above the globe, the ground is missing
    let c = globe(0.0);
    let v = c.view();

    assert!(c.ground(&v, 600.0, 400.0).is_some());
    assert!(c.ground(&v, 5.0, 5.0).is_none());
}

#[test]
fn pitch_past_the_horizon_keeps_a_finite_far_plane() {
    let mut c = camera();

    c.pitch = 80.0;
    c.clamp();
    assert_eq!(c.pitch, MAX_PITCH, "the default limit holds");
    c.max_pitch = 90.0;
    c.pitch = 89.0;
    c.clamp();
    assert_eq!(c.pitch, PITCH_LIMIT, "never past the hard limit");

    let view = c.view();

    assert!(view.far.is_finite() && view.far <= view.center_distance * FAR_LIMIT * 1.0001);
    // The top of the screen looks above the horizon: no ground there
    assert!(c.ground(&view, 600.0, 0.0).is_none());
    assert!(c.ground(&view, 600.0, 799.0).is_some());
}

#[test]
fn limits_from_the_page_never_panic() {
    let mut c = camera();

    // A minimum above the maximum: put in order
    c.set_zoom_range(15.0, 14.0);
    assert_eq!((c.min_zoom, c.max_zoom), (14.0, 15.0));
    assert!((14.0..=15.0).contains(&c.zoom));
    // A bound that is not a number is left as it was
    c.set_zoom_range(3.0, f64::NAN);
    assert_eq!((c.min_zoom, c.max_zoom), (3.0, 15.0));
    c.set_zoom_range(f64::NEG_INFINITY, 18.0);
    assert_eq!((c.min_zoom, c.max_zoom), (3.0, 18.0));
    c.set_max_pitch(f64::NAN);
    assert_eq!(c.max_pitch, MAX_PITCH);
    c.set_max_pitch(200.0);
    assert_eq!(c.max_pitch, PITCH_LIMIT);
    c.set_max_pitch(-5.0);
    assert_eq!(c.max_pitch, 0.0);
    // Fields set directly, out of order or not numbers: clamp still holds
    c.min_zoom = 10.0;
    c.max_zoom = 2.0;
    c.max_pitch = f64::NAN;
    c.zoom = f64::NAN;
    c.pitch = f64::INFINITY;
    c.bearing = f64::NAN;
    c.center = DVec2::new(f64::NAN, f64::INFINITY);
    c.clamp();
    assert!(c.zoom.is_finite() && c.pitch.is_finite() && c.bearing.is_finite());
    assert!(c.center.is_finite());
    c.globe = true;
    c.min_zoom = f64::NAN;
    c.clamp();
    assert!(c.zoom.is_finite());
}

#[test]
fn a_view_not_a_number_keeps_the_camera() {
    let mut c = camera();
    let before = c.clone();

    c.set_view(f64::NAN, 10.0, f64::NAN, f64::INFINITY, f64::NAN);
    assert_eq!(c.center, before.center);
    assert_eq!(
        (c.zoom, c.pitch, c.bearing),
        (before.zoom, before.pitch, before.bearing)
    );
    c.set_view(10.0, 20.0, 5.0, 30.0, 90.0);
    assert!((lon_lat(c.center) - DVec2::new(10.0, 20.0)).length() < 1e-9);
    assert_eq!((c.zoom, c.pitch, c.bearing), (5.0, 30.0, 90.0));
    assert!(c.view().view_proj.is_finite());
}
