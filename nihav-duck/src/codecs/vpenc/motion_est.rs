use nihav_codec_support::codecs::{MV, ZERO_MV};

use std::str::FromStr;

#[derive(Debug,Clone,Copy,PartialEq)]
pub enum MVSearchMode {
    Full,
    Diamond,
    Hexagon,
}

impl Default for MVSearchMode {
    fn default() -> Self { MVSearchMode::Hexagon }
}

pub struct ParseError{}

impl FromStr for MVSearchMode {
    type Err = ParseError;

    #[allow(clippy::single_match)]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "full"  => Ok(MVSearchMode::Full),
            "dia"   => Ok(MVSearchMode::Diamond),
            "hex"   => Ok(MVSearchMode::Hexagon),
            _ => Err(ParseError{}),
        }
    }
}

impl std::fmt::Display for MVSearchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match *self {
            MVSearchMode::Full      => write!(f, "full"),
            MVSearchMode::Diamond   => write!(f, "dia"),
            MVSearchMode::Hexagon   => write!(f, "hex"),
        }
    }
}

trait FromPixels {
    fn from_pixels(self) -> Self;
}

impl FromPixels for MV {
    fn from_pixels(self) -> MV {
        MV { x: self.x * 4, y: self.y * 4 }
    }
}

pub const DIA_PATTERN: [MV; 9] = [
    ZERO_MV,
    MV {x: -2, y:  0},
    MV {x: -1, y:  1},
    MV {x:  0, y:  2},
    MV {x:  1, y:  1},
    MV {x:  2, y:  0},
    MV {x:  1, y: -1},
    MV {x:  0, y: -2},
    MV {x: -1, y: -1}
];

pub const HEX_PATTERN: [MV; 7] = [
    ZERO_MV,
    MV {x: -2, y:  0},
    MV {x: -1, y:  2},
    MV {x:  1, y:  2},
    MV {x:  2, y:  0},
    MV {x:  1, y: -2},
    MV {x: -1, y: -2}
];

pub const REFINEMENT: [MV; 4] = [
    MV {x: -1, y:  0},
    MV {x:  0, y:  1},
    MV {x:  1, y:  0},
    MV {x:  0, y: -1}
];

#[macro_export]
macro_rules! search_template {
    ($self: expr, $mv_est: expr, $cur_blk: expr, $mb_x: expr, $mb_y: expr, $sad_func: ident) => ({
        let mut best_dist = MAX_DIST;
        let mut best_mv;

        let mut min_dist;
        let mut min_idx;

        $self.reset();
        loop {
            let mut cur_best_dist = best_dist;
            for (dist, &point) in $self.dist.iter_mut().zip($self.point.iter()) {
                if *dist == MAX_DIST {
                    *dist = $mv_est.$sad_func($cur_blk, $mb_x, $mb_y, point.from_pixels(), cur_best_dist);
                    cur_best_dist = cur_best_dist.min(*dist);
                    if *dist <= DIST_THRESH {
                        break;
                    }
                }
            }
            min_dist = $self.dist[0];
            min_idx = 0;
            for (i, &dist) in $self.dist.iter().enumerate().skip(1) {
                if dist < min_dist {
                    min_dist = dist;
                    min_idx = i;
                    if dist <= DIST_THRESH {
                        break;
                    }
                }
            }
            if min_dist <= DIST_THRESH || min_idx == 0 || best_dist == min_dist || $self.point[min_idx].x.abs() >= $mv_est.mv_range || $self.point[min_idx].y.abs() >= $mv_est.mv_range {
                break;
            }
            best_dist = min_dist;
            $self.update($self.steps[min_idx]);
        }
        best_dist = min_dist;
        best_mv   = $self.point[min_idx];
        if best_dist <= DIST_THRESH {
            return (best_mv.from_pixels(), best_dist);
        }
        for &step in REFINEMENT.iter() {
            let mv = best_mv + step;
            let dist = $mv_est.$sad_func($cur_blk, $mb_x, $mb_y, mv.from_pixels(), MAX_DIST);
            if best_dist > dist {
                best_dist = dist;
                best_mv = mv;
            }
        }
        best_mv = best_mv.from_pixels();
        if best_dist <= DIST_THRESH {
            return (best_mv, best_dist);
        }

        // subpel refinement
        $self.set_new_point(best_mv, best_dist);
        loop {
            let mut cur_best_dist = best_dist;
            for (dist, &point) in $self.dist.iter_mut().zip($self.point.iter()) {
                if *dist == MAX_DIST {
                    *dist = $mv_est.$sad_func($cur_blk, $mb_x, $mb_y, point, cur_best_dist);
                    cur_best_dist = cur_best_dist.min(*dist);
                    if *dist <= DIST_THRESH {
                        break;
                    }
                }
            }
            min_dist = $self.dist[0];
            min_idx = 0;
            for (i, &dist) in $self.dist.iter().enumerate().skip(1) {
                if dist < min_dist {
                    min_dist = dist;
                    min_idx = i;
                    if dist <= DIST_THRESH {
                        break;
                    }
                }
            }
            if min_dist <= DIST_THRESH || min_idx == 0 || best_dist == min_dist || $self.point[min_idx].x.abs() >= $mv_est.mv_range * 4 || $self.point[min_idx].y.abs() >= $mv_est.mv_range * 4 {
                break;
            }
            best_dist = min_dist;
            $self.update($self.steps[min_idx]);
        }
        best_dist = min_dist;
        best_mv   = $self.point[min_idx];
        if best_dist <= DIST_THRESH {
            return (best_mv, best_dist);
        }
        for &step in REFINEMENT.iter() {
            let mv = best_mv + step;
            let dist = $mv_est.$sad_func($cur_blk, $mb_x, $mb_y, mv, MAX_DIST);
            if best_dist > dist {
                best_dist = dist;
                best_mv = mv;
            }
        }
        (best_mv, best_dist)
    })
}
