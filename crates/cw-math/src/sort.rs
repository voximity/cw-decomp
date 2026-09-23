//! `std::sort` as MSVC 2012's STL (`msvcp110`) compiles it, so that elements the predicate
//! considers equal end up exactly where the original's sort leaves them. The instance the
//! server uses is `Server.exe 0x004f6080` (`_Sort` over 12-byte records, `_Unguarded_partition`
//! 0x004f6db0, `_Median` 0x004f5940, `_Med3` 0x004f50e0, `_Insertion_sort` 0x004f4ab0,
//! `_Make_heap` 0x004f4f30 and `_Sort_heap` 0x004f63f0), whose only caller takes the first
//! element after sorting, so ties matter.
//!
//! The algorithm is the VC11 introsort: quicksort with a median-of-three (median-of-nine over
//! 40 elements) pivot while the ideal depth budget lasts, insertion sort below 33 elements,
//! heap sort when the budget runs out. `pred(a, b)` is the strict "a before b" test.

/// Sorts `v` with `pred` as VC11's `std::sort(v.begin(), v.end(), pred)`.
pub fn msvc_sort<T: Clone, P: Fn(&T, &T) -> bool>(v: &mut [T], pred: P) {
    let n = v.len();
    sort_range(v, 0, n, n as isize, &pred);
}

/// `_ISORT_MAX`.
const ISORT_MAX: usize = 32;

fn sort_range<T: Clone, P: Fn(&T, &T) -> bool>(v: &mut [T], mut first: usize, mut last: usize, mut ideal: isize, pred: &P) {
    let mut count = last - first;
    while ISORT_MAX < count && 0 < ideal {
        let (mid_first, mid_second) = unguarded_partition(v, first, last, pred);
        ideal /= 2;
        ideal += ideal / 2;
        if mid_first - first < last - mid_second {
            sort_range(v, first, mid_first, ideal, pred);
            first = mid_second;
        } else {
            sort_range(v, mid_second, last, ideal, pred);
            last = mid_first;
        }
        count = last - first;
    }
    if ISORT_MAX < count {
        make_heap(v, first, last, pred);
        sort_heap(v, first, last, pred);
    } else if 1 < count {
        insertion_sort(v, first, last, pred);
    }
}

/// `_Med3`: the median of three elements to the middle.
fn med3<T: Clone, P: Fn(&T, &T) -> bool>(v: &mut [T], first: usize, mid: usize, last: usize, pred: &P) {
    if pred(&v[mid], &v[first]) {
        v.swap(mid, first);
    }
    if pred(&v[last], &v[mid]) {
        v.swap(last, mid);
        if pred(&v[mid], &v[first]) {
            v.swap(mid, first);
        }
    }
}

/// `_Median`: median of nine over 40 elements, of three otherwise.
fn median<T: Clone, P: Fn(&T, &T) -> bool>(v: &mut [T], first: usize, mid: usize, last: usize, pred: &P) {
    if 40 < last - first {
        let step = (last - first + 1) / 8;
        med3(v, first, first + step, first + 2 * step, pred);
        med3(v, mid - step, mid, mid + step, pred);
        med3(v, last - 2 * step, last - step, last, pred);
        med3(v, first + step, mid, last - step, pred);
    } else {
        med3(v, first, mid, last, pred);
    }
}

/// `_Unguarded_partition`: returns the `(pfirst, plast)` bounds of the pivot run.
fn unguarded_partition<T: Clone, P: Fn(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, pred: &P) -> (usize, usize) {
    let mid = first + (last - first) / 2;
    median(v, first, mid, last - 1, pred);
    let mut pfirst = mid;
    let mut plast = pfirst + 1;
    while first < pfirst && !pred(&v[pfirst - 1], &v[mid]) && !pred(&v[mid], &v[pfirst - 1]) {
        pfirst -= 1;
    }
    while plast < last && !pred(&v[plast], &v[mid]) && !pred(&v[mid], &v[plast]) {
        plast += 1;
    }
    let mut gfirst = plast;
    let mut glast = pfirst;
    loop {
        while gfirst < last {
            if pred(&v[pfirst], &v[gfirst]) {
            } else if pred(&v[gfirst], &v[pfirst]) {
                break;
            } else {
                v.swap(plast, gfirst);
                plast += 1;
            }
            gfirst += 1;
        }
        while first < glast {
            if pred(&v[glast - 1], &v[pfirst]) {
            } else if pred(&v[pfirst], &v[glast - 1]) {
                break;
            } else {
                pfirst -= 1;
                v.swap(pfirst, glast - 1);
            }
            glast -= 1;
        }
        if glast == first && gfirst == last {
            return (pfirst, plast);
        }
        if glast == first {
            // No room at the bottom: rotate the pivot upward.
            if plast != gfirst {
                v.swap(pfirst, plast);
            }
            plast += 1;
            v.swap(pfirst, gfirst);
            pfirst += 1;
            gfirst += 1;
        } else if gfirst == last {
            // No room at the top: rotate the pivot downward.
            glast -= 1;
            pfirst -= 1;
            if glast != pfirst {
                v.swap(glast, pfirst);
            }
            plast -= 1;
            v.swap(pfirst, plast);
        } else {
            glast -= 1;
            v.swap(gfirst, glast);
            gfirst += 1;
        }
    }
}

/// `_Insertion_sort`.
fn insertion_sort<T: Clone, P: Fn(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, pred: &P) {
    if first == last {
        return;
    }
    let mut next = first + 1;
    while next != last {
        let val = v[next].clone();
        if pred(&val, &v[first]) {
            // A new earliest element: shift everything up one.
            for i in (first..next).rev() {
                v[i + 1] = v[i].clone();
            }
            v[first] = val;
        } else {
            let mut hole = next;
            while pred(&val, &v[hole - 1]) {
                v[hole] = v[hole - 1].clone();
                hole -= 1;
            }
            v[hole] = val;
        }
        next += 1;
    }
}

/// `_Push_heap`.
fn push_heap<T: Clone, P: Fn(&T, &T) -> bool>(v: &mut [T], first: usize, mut hole: usize, top: usize, val: T, pred: &P) {
    while top < hole {
        let idx = (hole - 1) / 2;
        if !pred(&v[first + idx], &val) {
            break;
        }
        v[first + hole] = v[first + idx].clone();
        hole = idx;
    }
    v[first + hole] = val;
}

/// `_Adjust_heap`.
fn adjust_heap<T: Clone, P: Fn(&T, &T) -> bool>(v: &mut [T], first: usize, mut hole: usize, bottom: usize, val: T, pred: &P) {
    let top = hole;
    let mut idx = 2 * hole + 2;
    while idx < bottom {
        if pred(&v[first + idx], &v[first + idx - 1]) {
            idx -= 1;
        }
        v[first + hole] = v[first + idx].clone();
        hole = idx;
        idx = 2 * idx + 2;
    }
    if idx == bottom {
        v[first + hole] = v[first + bottom - 1].clone();
        hole = bottom - 1;
    }
    push_heap(v, first, hole, top, val, pred);
}

/// `make_heap`.
fn make_heap<T: Clone, P: Fn(&T, &T) -> bool>(v: &mut [T], first: usize, last: usize, pred: &P) {
    let bottom = last - first;
    let mut hole = bottom / 2;
    while 0 < hole {
        hole -= 1;
        let val = v[first + hole].clone();
        adjust_heap(v, first, hole, bottom, val, pred);
    }
}

/// `sort_heap`: `pop_heap` until one element is left.
fn sort_heap<T: Clone, P: Fn(&T, &T) -> bool>(v: &mut [T], first: usize, mut last: usize, pred: &P) {
    while 1 < last - first {
        last -= 1;
        let val = v[last].clone();
        v[last] = v[first].clone();
        adjust_heap(v, first, 0, last - first, val, pred);
    }
}

#[cfg(test)]
mod tests {
    use super::msvc_sort;

    #[test]
    fn sorts_descending_with_stable_small_runs() {
        // Below 33 elements the insertion sort keeps equal elements in input order.
        let mut v: Vec<(i32, f32)> = (0..20).map(|i| (i, (i % 4) as f32)).collect();
        msvc_sort(&mut v, |a, b| a.1 > b.1);
        let expect: Vec<(i32, f32)> = [3, 2, 1, 0].iter().flat_map(|&w| (0..20).filter(move |i| i % 4 == w).map(move |i| (i, w as f32))).collect();
        assert_eq!(v, expect);
    }

    #[test]
    fn sorts_large_inputs() {
        let mut state = 12345u32;
        let mut v: Vec<(u32, f32)> = (0..1000)
            .map(|i| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                (i, (state >> 8) as f32)
            })
            .collect();
        msvc_sort(&mut v, |a, b| a.1 > b.1);
        assert!(v.windows(2).all(|w| w[0].1 >= w[1].1));
        // The heap path, with an exhausted depth budget.
        let mut w: Vec<u32> = (0..200).rev().collect();
        super::sort_range(&mut w, 0, 200, 0, &|a: &u32, b: &u32| a < b);
        assert!(w.windows(2).all(|p| p[0] <= p[1]));
    }
}
