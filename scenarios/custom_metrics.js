// Test scenario for Custom Metrics

export const options = {
    stages: [
        { duration: '2s', target: 5 },
    ],
    thresholds: {
        'processing_time': ['p95 < 200', 'avg < 100'],
        'items_processed': ['count > 10'],
        'success_rate': ['rate > 0.9'],
        'queue_size': ['value < 100'],
    }
};

export default function () {
    let start = Date.now();

    // Simulate some work
    sleep(0.05 + (Math.random() * 0.05));

    // Record histogram - tracks distribution of values
    let duration = Date.now() - start;
    metrics.histogramAdd('processing_time', duration);

    // Record counter - cumulative count
    metrics.counterAdd('items_processed', 1);

    // Record rate (occasional failure) - tracks success percentage
    let isSuccess = Math.random() > 0.05;
    metrics.rateAdd('success_rate', isSuccess);

    // Record gauge (varying value) - tracks latest value
    metrics.gaugeSet('queue_size', Math.floor(Math.random() * 50));
}
