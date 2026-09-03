import Foundation

struct ControllerRetryPolicy: Sendable {
    static let live = Self(
        maxAttempts: 8,
        maxElapsedSeconds: 90,
        baseDelaySeconds: 1,
        maxDelaySeconds: 30,
        randomUnit: { Double.random(in: 0..<1) },
        sleep: { seconds in
            try await Task.sleep(for: .seconds(seconds))
        }
    )

    let maxAttempts: Int
    let maxElapsedSeconds: TimeInterval
    let baseDelaySeconds: TimeInterval
    let maxDelaySeconds: TimeInterval
    private let randomUnit: @Sendable () -> Double
    private let sleepImplementation: @Sendable (TimeInterval) async throws -> Void

    init(
        maxAttempts: Int,
        maxElapsedSeconds: TimeInterval,
        baseDelaySeconds: TimeInterval,
        maxDelaySeconds: TimeInterval,
        randomUnit: @escaping @Sendable () -> Double,
        sleep: @escaping @Sendable (TimeInterval) async throws -> Void
    ) {
        self.maxAttempts = max(1, maxAttempts)
        self.maxElapsedSeconds = max(0, maxElapsedSeconds)
        self.baseDelaySeconds = max(0, baseDelaySeconds)
        self.maxDelaySeconds = max(0, maxDelaySeconds)
        self.randomUnit = randomUnit
        self.sleepImplementation = sleep
    }

    func delayAfterFailure(attempt: Int, elapsedSeconds: TimeInterval) -> TimeInterval? {
        guard attempt < maxAttempts, elapsedSeconds < maxElapsedSeconds else { return nil }
        let exponent = min(max(attempt - 1, 0), 20)
        let ceiling = min(baseDelaySeconds * pow(2, Double(exponent)), maxDelaySeconds)
        let remaining = maxElapsedSeconds - elapsedSeconds
        guard ceiling > 0, remaining > 0 else { return nil }
        let unit = min(max(randomUnit(), 0), 1)
        return min(ceiling * unit, remaining)
    }

    func sleep(for seconds: TimeInterval) async throws {
        try await sleepImplementation(seconds)
    }
}
