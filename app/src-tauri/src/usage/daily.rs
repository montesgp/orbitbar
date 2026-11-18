//! Daily usage buckets, shared by the readers whose entries carry their own
//! timestamps (claude and pi). A past day never changes for them, so they
//! aggregate into `(local date, project key, model)` buckets of raw counters,
//! and a window report is a fold over the dates in the window. Keeping the
//! raw counters (not a priced total) is what lets a bucket be stored, merged
//! with a stored copy, and priced again with the current price table; keeping
//! one bucket per model is what lets each model be priced at its own rate.
//! The ledger merge works on whole `(date, project)` groups (every model of
//! one project on one day), so a group has its own helpers.
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
    /// The bucket's model, part of its key; `""` when the entries named none.
    pub model: String,
}

impl DailyCounters {
    /// Adds `other`'s counters; the model is left as is (a bucket only ever
    /// adds entries of its own model).
    pub fn add(&mut self, other: &DailyCounters) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write_5m += other.cache_write_5m;
        self.cache_write_1h += other.cache_write_1h;
        self.reasoning += other.reasoning;
        self.entries += other.entries;
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

/// Every bucket a reader found, ordered by date, project key and model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DailyBuckets {
    buckets: BTreeMap<(NaiveDate, String, String), DailyCounters>,
}

/// A window's totals and per-project rows (unsorted; `AgentUsageReport::ok`
/// sorts them).
#[derive(Debug, Clone, PartialEq)]
pub struct WindowFold {
    pub totals: UsageTotals,
    pub projects: Vec<ProjectUsage>,
}

impl DailyBuckets {
    /// Adds `entry` to the `(date, project, entry.model)` bucket, creating it
    /// when missing.
    pub fn add(&mut self, date: NaiveDate, project: &str, entry: &DailyCounters) {
        self.buckets
            .entry((date, project.to_string(), entry.model.clone()))
            .or_insert_with(|| DailyCounters { model: entry.model.clone(), ..Default::default() })
            .add(entry);
    }

    /// Replaces the `(date, project, counters.model)` bucket, returning the
    /// previous one, so a stored copy can be put back.
    pub fn insert(&mut self, date: NaiveDate, project: &str, counters: DailyCounters) -> Option<DailyCounters> {
        self.buckets.insert((date, project.to_string(), counters.model.clone()), counters)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get(&self, date: NaiveDate, project: &str, model: &str) -> Option<&DailyCounters> {
        self.buckets.get(&(date, project.to_string(), model.to_string()))
    }

    /// Every model bucket of the `(date, project)` group, in model order.
    pub fn group(&self, date: NaiveDate, project: &str) -> impl Iterator<Item = &DailyCounters> {
        // `""` is the smallest model, so the range starts at the group's first bucket.
        let project = project.to_string();
        self.buckets
            .range((date, project.clone(), String::new())..)
            .take_while(move |((d, p, _), _)| *d == date && *p == project)
            .map(|(_, c)| c)
    }

    /// Whether any model has a bucket for `(date, project)`.
    pub fn contains(&self, date: NaiveDate, project: &str) -> bool {
        self.group(date, project).next().is_some()
    }

    /// The entries of every model in the `(date, project)` group; 0 when it has none.
    pub fn group_entries(&self, date: NaiveDate, project: &str) -> u64 {
        self.group(date, project).map(|c| c.entries).sum()
    }

    /// Replaces every model bucket of the `(date, project)` group with `counters`.
    pub fn replace_group<'c>(
        &mut self,
        date: NaiveDate,
        project: &str,
        counters: impl IntoIterator<Item = &'c DailyCounters>,
    ) {
        let models: Vec<String> = self.group(date, project).map(|c| c.model.clone()).collect();
        for model in models {
            self.buckets.remove(&(date, project.to_string(), model));
        }
        for c in counters {
            self.insert(date, project, c.clone());
        }
    }

    /// Every bucket, in date order, then project-key order, then model order.
    pub fn iter(&self) -> impl Iterator<Item = (NaiveDate, &str, &DailyCounters)> {
        self.buckets.iter().map(|((date, project, _), c)| (*date, project.as_str(), c))
    }

    /// Every `(date, project)` group that has a bucket, once each, in order.
    pub fn groups(&self) -> impl Iterator<Item = (NaiveDate, &str)> {
        let mut last: Option<(NaiveDate, &str)> = None;
        self.iter().filter_map(move |(date, project, _)| {
            let key = Some((date, project));
            if last == key {
                return None;
            }
            last = key;
            key
        })
    }

    /// Folds the buckets dated in `[start, end)` into a window report. Each
    /// bucket is priced on its own with `price` (its own model, its own
    /// counters) and the amounts are summed, so a model change between days
    /// never reprices the earlier days and two models of one day are priced
    /// each at its own rate. A total's cost is `None` only when no bucket in
    /// it could be priced; `cost_basis` is `basis` exactly when `cost` is set.
    /// A project row's model is the named model with the most output tokens
    /// in the window; on a tie the one used last, then the greater name.
    pub fn fold(
        &self,
        start: NaiveDate,
        end: NaiveDate,
        price: impl Fn(&DailyCounters) -> Option<f64>,
        basis: CostBasis,
    ) -> WindowFold {
        let mut totals = Cost::default();
        let mut projects: BTreeMap<&str, (Cost, ModelOutputs)> = BTreeMap::new();
        if start < end {
            // `(end, "", "")` is the smallest key dated `end`, so the range
            // stops right before that day.
            let range = (start, String::new(), String::new())..(end, String::new(), String::new());
            for ((date, project, model), counters) in self.buckets.range(range) {
                let amount = price(counters);
                totals.add(counters, amount);
                if project.is_empty() {
                    continue;
                }
                let (row, models) = projects.entry(project.as_str()).or_default();
                row.add(counters, amount);
                if !model.is_empty() {
                    // Dates ascend, so the last date seen is the latest.
                    let (output, last) = models.entry(model.as_str()).or_insert((0, *date));
                    *output += counters.output;
                    *last = *date;
                }
            }
        }

        WindowFold {
            totals: totals.finish(basis),
            projects: projects
                .into_iter()
                .map(|(path, (row, models))| ProjectUsage {
                    path: path.to_string(),
                    name: project_display_name(path),
                    model: models
                        .into_iter()
                        .max_by_key(|(model, (output, last))| (*output, *last, *model))
                        .map(|(model, _)| model.to_string())
                        .unwrap_or_default(),
                    totals: row.finish(basis),
                })
                .collect(),
        }
    }
}

/// Per named model of a project row: its output tokens and the last date it
/// was used, which pick the row's model.
type ModelOutputs<'a> = BTreeMap<&'a str, (u64, NaiveDate)>;

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
    fn add_sums_counters_per_model() {
        let mut buckets = DailyBuckets::default();
        buckets.add(day(1), "p", &entry("a", 5));
        buckets.add(day(1), "p", &entry("a", 7));
        buckets.add(day(1), "p", &entry("", 1));

        let got = buckets.get(day(1), "p", "a").expect("bucket exists");
        assert_eq!(
            *got,
            DailyCounters {
                input: 20,
                output: 12,
                cache_read: 2,
                cache_write_5m: 4,
                cache_write_1h: 6,
                reasoning: 8,
                entries: 2,
                model: "a".into(),
            }
        );
        assert_eq!(buckets.get(day(1), "p", ""), Some(&entry("", 1)), "a model-less entry has its own bucket");
        assert!(buckets.contains(day(1), "p"));
        assert!(!buckets.contains(day(2), "p"));
        assert!(!buckets.contains(day(1), "q"));
        assert_eq!(buckets.group_entries(day(1), "p"), 3);
        assert_eq!(buckets.group_entries(day(1), "q"), 0);
        assert_eq!(buckets.iter().count(), 2);
    }

    #[test]
    fn insert_replaces_a_model_bucket_and_returns_the_previous_one() {
        let mut buckets = DailyBuckets::default();
        assert_eq!(buckets.insert(day(1), "p", entry("a", 5)), None);
        assert_eq!(buckets.insert(day(1), "p", entry("a", 9)), Some(entry("a", 5)));
        assert_eq!(buckets.insert(day(1), "p", entry("b", 1)), None, "another model is another bucket");
        assert_eq!(buckets.get(day(1), "p", "a"), Some(&entry("a", 9)));
        let keys: Vec<(NaiveDate, &str)> = buckets.iter().map(|(d, p, _)| (d, p)).collect();
        assert_eq!(keys, vec![(day(1), "p"), (day(1), "p")]);
    }

    /// A group is every model of one (date, project); its neighbours, even a
    /// project key that starts with the same text, are not part of it.
    #[test]
    fn groups_cover_every_model_of_one_date_and_project() {
        let mut buckets = DailyBuckets::default();
        buckets.add(day(1), "p", &entry("a", 1));
        buckets.add(day(1), "p", &entry("b", 2));
        buckets.add(day(1), "p2", &entry("a", 4));
        buckets.add(day(2), "p", &entry("a", 8));

        let outputs: Vec<u64> = buckets.group(day(1), "p").map(|c| c.output).collect();
        assert_eq!(outputs, vec![1, 2]);
        assert_eq!(buckets.groups().collect::<Vec<_>>(), vec![(day(1), "p"), (day(1), "p2"), (day(2), "p")]);

        buckets.replace_group(day(1), "p", [&entry("c", 16)]);
        let models: Vec<&str> = buckets.group(day(1), "p").map(|c| c.model.as_str()).collect();
        assert_eq!(models, vec!["c"]);
        assert_eq!(buckets.iter().count(), 3, "the other groups are untouched");
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
        assert_eq!(p.model, "unknown", "the model with the most output names the row");

        let q = fold.projects.iter().find(|r| r.path == "q").expect("row q");
        assert_eq!(q.totals.cost, None, "no bucket of q could be priced");
        assert_eq!(q.totals.cost_basis, None);
    }

    /// Rates per output token: "cheap" 1, "dear" 3, anything else unpriced.
    fn rate(c: &DailyCounters) -> Option<f64> {
        match c.model.as_str() {
            "cheap" => Some(c.output as f64),
            "dear" => Some(3.0 * c.output as f64),
            _ => None,
        }
    }

    /// Two models in one (date, project) stay apart and each is priced at its
    /// own rate, not the whole day at the last model's.
    #[test]
    fn a_day_mixing_two_models_prices_each_at_its_own_rate() {
        let mut buckets = DailyBuckets::default();
        buckets.add(day(1), "p", &entry("cheap", 10));
        buckets.add(day(1), "p", &entry("dear", 5));
        buckets.add(day(1), "p", &entry("cheap", 2));

        assert_eq!(buckets.iter().count(), 2, "one bucket per model");
        let fold = buckets.fold(day(1), day(2), rate, CostBasis::ApiEquivalent);
        assert_eq!(fold.totals.output_tokens, 17);
        assert_eq!(fold.totals.entries, 3);
        assert_eq!(fold.totals.cost, Some(12.0 + 15.0), "12 cheap at 1 plus 5 dear at 3");
        assert_eq!(fold.projects.len(), 1);
        assert_eq!(fold.projects[0].totals, fold.totals);
    }

    /// A row names the model with the most output tokens in the window, not
    /// the latest one; a model-less bucket never names it.
    #[test]
    fn a_project_row_names_the_model_with_the_most_output() {
        let mut buckets = DailyBuckets::default();
        buckets.add(day(1), "p", &entry("big", 60));
        buckets.add(day(2), "p", &entry("big", 50));
        buckets.add(day(2), "p", &entry("small", 100));
        buckets.add(day(3), "p", &entry("", 1000));

        let fold = buckets.fold(day(1), day(4), price, CostBasis::ApiEquivalent);
        assert_eq!(fold.projects[0].model, "big", "110 output across two days beats 100");
        let only_day_two = buckets.fold(day(2), day(3), price, CostBasis::ApiEquivalent);
        assert_eq!(only_day_two.projects[0].model, "small", "counted over the window only");
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
