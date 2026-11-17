//! Daily usage buckets, shared by the readers whose entries carry their own
//! timestamps (claude, and later pi). A past day never changes for them, so
//! they aggregate into `(local date, project key)` buckets of raw counters,
//! and a window report is a fold over the dates in the window. Keeping the
//! raw counters (not a priced total) is what lets a bucket be stored, merged
//! with a stored copy, and priced again with the current price table.
//!
//! `""` is the no-project key: an entry with no cwd counts in the agent's
//! totals but has no project row.

use crate::usage::{project_display_name, CostBasis, ProjectUsage, UsageTotals};
use chrono::NaiveDate;
use std::collections::BTreeMap;

/// Raw counters for one bucket (or one entry, with `entries: 1`). Cache
/// writes stay split by TTL because Anthropic prices a 5-minute write
/// differently from a 1-hour one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DailyCounters {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub reasoning: u64,
    pub entries: u64,
    /// The model of the last added entry that named one; `""` when none did.
    pub model: String,
}

impl DailyCounters {
    /// Adds `other`'s counters; its model replaces this one unless it is empty.
    pub fn add(&mut self, other: &DailyCounters) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write_5m += other.cache_write_5m;
        self.cache_write_1h += other.cache_write_1h;
        self.reasoning += other.reasoning;
        self.entries += other.entries;
        if !other.model.is_empty() {
            self.model.clone_from(&other.model);
        }
    }

    fn add_to(&self, totals: &mut UsageTotals) {
        totals.input_tokens += self.input;
        totals.output_tokens += self.output;
        totals.cache_read_tokens += self.cache_read;
        totals.cache_write_tokens += self.cache_write_5m + self.cache_write_1h;
        totals.reasoning_tokens += self.reasoning;
        totals.entries += self.entries;
    }
}

/// Every bucket a reader found, ordered by date and then project key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DailyBuckets {
    buckets: BTreeMap<(NaiveDate, String), DailyCounters>,
}

/// A window's totals and per-project rows (unsorted; `AgentUsageReport::ok`
/// sorts them).
#[derive(Debug, Clone, PartialEq)]
pub struct WindowFold {
    pub totals: UsageTotals,
    pub projects: Vec<ProjectUsage>,
}

impl DailyBuckets {
    /// Adds `entry` to the `(date, project)` bucket, creating it when missing.
    pub fn add(&mut self, date: NaiveDate, project: &str, entry: &DailyCounters) {
        self.buckets.entry((date, project.to_string())).or_default().add(entry);
    }

    /// Replaces the `(date, project)` bucket, returning the previous one, so
    /// a stored copy can be merged in.
    #[allow(dead_code)] // first caller: the usage ledger
    pub fn insert(&mut self, date: NaiveDate, project: &str, counters: DailyCounters) -> Option<DailyCounters> {
        self.buckets.insert((date, project.to_string()), counters)
    }

    pub fn get(&self, date: NaiveDate, project: &str) -> Option<&DailyCounters> {
        self.buckets.get(&(date, project.to_string()))
    }

    #[allow(dead_code)] // first caller: the Pi reader's claude-bridge check
    pub fn contains(&self, date: NaiveDate, project: &str) -> bool {
        self.get(date, project).is_some()
    }

    /// Every bucket, in date order and then project-key order.
    #[allow(dead_code)] // first caller: the usage ledger
    pub fn iter(&self) -> impl Iterator<Item = (NaiveDate, &str, &DailyCounters)> {
        self.buckets.iter().map(|((date, project), c)| (*date, project.as_str(), c))
    }

    /// Folds the buckets dated in `[start, end)` into a window report. Each
    /// bucket is priced on its own with `price` (its own model, its own
    /// counters) and the amounts are summed, so a model change between days
    /// never reprices the earlier days. A total's cost is `None` only when
    /// no bucket in it could be priced; `cost_basis` is `basis` exactly when
    /// `cost` is set. A project row's model is the one its latest dated
    /// bucket names.
    pub fn fold(
        &self,
        start: NaiveDate,
        end: NaiveDate,
        price: impl Fn(&DailyCounters) -> Option<f64>,
        basis: CostBasis,
    ) -> WindowFold {
        let mut totals = Cost::default();
        let mut projects: BTreeMap<&str, (Cost, &str)> = BTreeMap::new();
        if start < end {
            // `(end, "")` is the smallest key dated `end`, so the range stops
            // right before that day.
            for ((_, project), counters) in self.buckets.range((start, String::new())..(end, String::new())) {
                let amount = price(counters);
                totals.add(counters, amount);
                if project.is_empty() {
                    continue;
                }
                let (row, model) = projects.entry(project.as_str()).or_insert_with(|| (Cost::default(), ""));
                row.add(counters, amount);
                if !counters.model.is_empty() {
                    *model = counters.model.as_str();
                }
            }
        }

        WindowFold {
            totals: totals.finish(basis),
            projects: projects
                .into_iter()
                .map(|(path, (row, model))| ProjectUsage {
                    path: path.to_string(),
                    name: project_display_name(path),
                    model: model.to_string(),
                    totals: row.finish(basis),
                })
                .collect(),
        }
    }
}

/// Totals being folded, with the summed amount of the buckets that had a price.
#[derive(Default)]
struct Cost {
    totals: UsageTotals,
    amount: Option<f64>,
}

impl Cost {
    fn add(&mut self, counters: &DailyCounters, amount: Option<f64>) {
        counters.add_to(&mut self.totals);
        if let Some(amount) = amount {
            *self.amount.get_or_insert(0.0) += amount;
        }
    }

    /// Rounds the sum to the 4 decimals each priced bucket carries, so float
    /// noise from the addition never shows.
    fn finish(self, basis: CostBasis) -> UsageTotals {
        let cost = self.amount.map(|a| (a * 10_000.0).round() / 10_000.0);
        UsageTotals { cost, cost_basis: cost.map(|_| basis), ..self.totals }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, d).unwrap()
    }

    fn entry(model: &str, output: u64) -> DailyCounters {
        DailyCounters {
            input: 10,
            output,
            cache_read: 1,
            cache_write_5m: 2,
            cache_write_1h: 3,
            reasoning: 4,
            entries: 1,
            model: model.to_string(),
        }
    }

    /// One unit of money per output token for "priced", nothing for any other model.
    fn price(c: &DailyCounters) -> Option<f64> {
        (c.model == "priced").then_some(c.output as f64)
    }

    #[test]
    fn add_sums_counters_and_keeps_the_last_named_model() {
        let mut buckets = DailyBuckets::default();
        buckets.add(day(1), "p", &entry("a", 5));
        buckets.add(day(1), "p", &entry("b", 7));
        buckets.add(day(1), "p", &entry("", 1));

        let got = buckets.get(day(1), "p").expect("bucket exists");
        assert_eq!(
            *got,
            DailyCounters {
                input: 30,
                output: 13,
                cache_read: 3,
                cache_write_5m: 6,
                cache_write_1h: 9,
                reasoning: 12,
                entries: 3,
                model: "b".into(),
            }
        );
        assert!(buckets.contains(day(1), "p"));
        assert!(!buckets.contains(day(2), "p"));
        assert!(!buckets.contains(day(1), "q"));
        assert_eq!(buckets.iter().count(), 1);
    }

    #[test]
    fn insert_replaces_a_bucket_and_returns_the_previous_one() {
        let mut buckets = DailyBuckets::default();
        assert_eq!(buckets.insert(day(1), "p", entry("a", 5)), None);
        assert_eq!(buckets.insert(day(1), "p", entry("b", 9)), Some(entry("a", 5)));
        assert_eq!(buckets.get(day(1), "p"), Some(&entry("b", 9)));
        let keys: Vec<(NaiveDate, &str)> = buckets.iter().map(|(d, p, _)| (d, p)).collect();
        assert_eq!(keys, vec![(day(1), "p")]);
    }

    #[test]
    fn fold_counts_only_the_dates_in_the_half_open_window() {
        let mut buckets = DailyBuckets::default();
        buckets.add(day(1), "p", &entry("priced", 100)); // before
        buckets.add(day(2), "p", &entry("priced", 10));
        buckets.add(day(3), "p", &entry("priced", 20));
        buckets.add(day(4), "p", &entry("priced", 1000)); // end, excluded

        let fold = buckets.fold(day(2), day(4), price, CostBasis::ApiEquivalent);
        assert_eq!(fold.totals.output_tokens, 30);
        assert_eq!(fold.totals.input_tokens, 20);
        assert_eq!(fold.totals.cache_read_tokens, 2);
        assert_eq!(fold.totals.cache_write_tokens, 10, "5m plus 1h");
        assert_eq!(fold.totals.reasoning_tokens, 8);
        assert_eq!(fold.totals.entries, 2);
        assert_eq!(fold.totals.cost, Some(30.0));
        assert_eq!(fold.totals.cost_basis, Some(CostBasis::ApiEquivalent));
        assert_eq!(fold.projects.len(), 1);
        assert_eq!(fold.projects[0].totals, fold.totals);
    }

    /// Each bucket is priced with its own model: an unpriced bucket adds its
    /// tokens but no cost, and never hides the cost of the priced ones.
    #[test]
    fn fold_prices_each_bucket_and_an_unpriced_one_adds_no_cost() {
        let mut buckets = DailyBuckets::default();
        buckets.add(day(1), "p", &entry("priced", 10));
        buckets.add(day(2), "p", &entry("unknown", 500)); // latest model is unpriced
        buckets.add(day(2), "q", &entry("unknown", 7));

        let fold = buckets.fold(day(1), day(3), price, CostBasis::ApiEquivalent);
        assert_eq!(fold.totals.output_tokens, 517);
        assert_eq!(fold.totals.cost, Some(10.0), "only the priced bucket costs");
        assert_eq!(fold.totals.cost_basis, Some(CostBasis::ApiEquivalent));

        let p = fold.projects.iter().find(|r| r.path == "p").expect("row p");
        assert_eq!(p.totals.output_tokens, 510);
        assert_eq!(p.totals.cost, Some(10.0));
        assert_eq!(p.model, "unknown", "the latest day's model names the row");

        let q = fold.projects.iter().find(|r| r.path == "q").expect("row q");
        assert_eq!(q.totals.cost, None, "no bucket of q could be priced");
        assert_eq!(q.totals.cost_basis, None);
    }

    #[test]
    fn fold_with_no_priced_bucket_has_no_cost() {
        let mut buckets = DailyBuckets::default();
        buckets.add(day(1), "p", &entry("unknown", 10));

        let fold = buckets.fold(day(1), day(2), price, CostBasis::ApiEquivalent);
        assert_eq!(fold.totals.output_tokens, 10);
        assert_eq!(fold.totals.cost, None);
        assert_eq!(fold.totals.cost_basis, None);
    }

    /// The no-project key counts in the totals and never becomes a row.
    #[test]
    fn fold_keeps_the_no_project_bucket_in_totals_only() {
        let mut buckets = DailyBuckets::default();
        buckets.add(day(1), "", &entry("priced", 5));
        buckets.add(day(1), "C:\\repos\\orbitbar", &entry("priced", 10));

        let fold = buckets.fold(day(1), day(2), price, CostBasis::Reported);
        assert_eq!(fold.totals.output_tokens, 15);
        assert_eq!(fold.totals.entries, 2);
        assert_eq!(fold.totals.cost, Some(15.0));
        assert_eq!(fold.totals.cost_basis, Some(CostBasis::Reported));
        assert_eq!(fold.projects.len(), 1);
        assert_eq!(fold.projects[0].path, "C:\\repos\\orbitbar");
        assert_eq!(fold.projects[0].name, "orbitbar");
        assert_eq!(fold.projects[0].totals.output_tokens, 10);
    }

    #[test]
    fn fold_of_an_empty_window_is_all_zero() {
        let mut buckets = DailyBuckets::default();
        buckets.add(day(1), "p", &entry("priced", 10));

        let fold = buckets.fold(day(5), day(6), price, CostBasis::ApiEquivalent);
        assert_eq!(fold.totals, UsageTotals::default());
        assert!(fold.projects.is_empty());
    }
}
