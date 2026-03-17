use std::collections::HashSet;

use crate::constants::{INVALID_METRIC_VALUE, METRIC_VALUES_CAPACITY};
use crate::errors::CoreError;
use crate::types::CollectorMetadata;

pub fn validate_non_empty(field: &'static str, value: &str) -> Result<(), CoreError> {
    if value.trim().is_empty() {
        return Err(CoreError::EmptyField(field));
    }
    Ok(())
}

pub fn normalize_metric_values(mut values: Vec<i64>) -> Result<Vec<i64>, CoreError> {
    if values.len() > METRIC_VALUES_CAPACITY {
        return Err(CoreError::TooManyMetricValues {
            got: values.len(),
            max: METRIC_VALUES_CAPACITY,
        });
    }

    values.resize(METRIC_VALUES_CAPACITY, INVALID_METRIC_VALUE);
    Ok(values)
}

pub fn validate_collectors(collectors: &[CollectorMetadata]) -> Result<(), CoreError> {
    if collectors.is_empty() {
        return Err(CoreError::NoCollectors);
    }

    let mut seen_orders = HashSet::with_capacity(collectors.len());
    let mut seen_keys = HashSet::with_capacity(collectors.len());

    for collector in collectors {
        if !seen_orders.insert(collector.order) {
            return Err(CoreError::DuplicateCollectorOrder {
                order: collector.order,
            });
        }

        if !seen_keys.insert(collector.collector_key.as_str()) {
            return Err(CoreError::DuplicateCollectorKey {
                key: collector.collector_key.clone(),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::constants::{INVALID_METRIC_VALUE, METRIC_VALUES_CAPACITY};

    use super::normalize_metric_values;

    #[test]
    fn normalize_metric_values_pads_with_missing_value() {
        let values = normalize_metric_values(vec![1, 2, 3]).expect("values should be normalized");
        assert_eq!(values.len(), METRIC_VALUES_CAPACITY);
        assert_eq!(values[3], INVALID_METRIC_VALUE);
    }

    #[test]
    fn normalize_metric_values_rejects_oversize_input() {
        let oversize = vec![1; METRIC_VALUES_CAPACITY + 1];
        assert!(normalize_metric_values(oversize).is_err());
    }
}
