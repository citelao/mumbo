// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, renderHook } from "@testing-library/react";
import { throttled, useThrottledEffect, useTimeout } from "./throttle";

beforeEach(() => {
    vi.useFakeTimers();
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

function renderThrottled(initialValue: number, intervalMs = 1000) {
    const effect = vi.fn<(value: number, signal: AbortSignal) => void>();
    const hook = renderHook(
        ({ value }) => useThrottledEffect(signal => effect(value, signal), [value], intervalMs),
        { initialProps: { value: initialValue } },
    );
    return { effect, ...hook };
}

describe("useThrottledEffect", () => {
    it("runs immediately on mount", () => {
        const { effect } = renderThrottled(1);

        expect(effect).toHaveBeenCalledTimes(1);
        expect(effect).toHaveBeenLastCalledWith(1, expect.any(AbortSignal));
    });

    it("defers a change inside the window until the window ends", () => {
        const { effect, rerender } = renderThrottled(1);

        vi.advanceTimersByTime(300);
        rerender({ value: 2 });
        expect(effect).toHaveBeenCalledTimes(1);

        vi.advanceTimersByTime(699);
        expect(effect).toHaveBeenCalledTimes(1);

        vi.advanceTimersByTime(1);
        expect(effect).toHaveBeenCalledTimes(2);
        expect(effect).toHaveBeenLastCalledWith(2, expect.any(AbortSignal));
    });

    it("collapses several changes inside the window into one trailing run with the latest value", () => {
        const { effect, rerender } = renderThrottled(1);

        rerender({ value: 2 });
        vi.advanceTimersByTime(100);
        rerender({ value: 3 });
        vi.advanceTimersByTime(100);
        rerender({ value: 4 });

        vi.advanceTimersByTime(1000);
        expect(effect).toHaveBeenCalledTimes(2);
        expect(effect).toHaveBeenLastCalledWith(4, expect.any(AbortSignal));
    });

    it("runs immediately when a change arrives after the window", () => {
        const { effect, rerender } = renderThrottled(1);

        vi.advanceTimersByTime(1000);
        rerender({ value: 2 });

        expect(effect).toHaveBeenCalledTimes(2);
        expect(effect).toHaveBeenLastCalledWith(2, expect.any(AbortSignal));
    });

    it("does not rerun when deps are unchanged", () => {
        const { effect, rerender } = renderThrottled(1);

        rerender({ value: 1 });
        vi.advanceTimersByTime(5000);

        expect(effect).toHaveBeenCalledTimes(1);
    });

    it("aborts the previous run's signal when a newer run starts", () => {
        const { effect, rerender } = renderThrottled(1);
        const firstSignal = effect.mock.calls[0][1];

        rerender({ value: 2 });
        expect(firstSignal.aborted).toBe(false);

        vi.advanceTimersByTime(1000);
        expect(firstSignal.aborted).toBe(true);
        expect(effect.mock.calls[1][1].aborted).toBe(false);
    });

    it("cancels a pending trailing run and aborts the signal on unmount", () => {
        const { effect, rerender, unmount } = renderThrottled(1);
        const firstSignal = effect.mock.calls[0][1];

        rerender({ value: 2 });
        unmount();
        vi.advanceTimersByTime(5000);

        expect(effect).toHaveBeenCalledTimes(1);
        expect(firstSignal.aborted).toBe(true);
    });
});

describe("throttled", () => {
    it("runs the first call immediately and returns its result", async () => {
        const fn = vi.fn(async () => "ok");
        const run = throttled(fn, 1000);

        await expect(run()).resolves.toBe("ok");
        expect(fn).toHaveBeenCalledTimes(1);
    });

    it("delays a call inside the window until the window ends", async () => {
        const fn = vi.fn(async () => {});
        const run = throttled(fn, 1000);

        await run();
        vi.advanceTimersByTime(300);
        const second = run();

        await vi.advanceTimersByTimeAsync(699);
        expect(fn).toHaveBeenCalledTimes(1);

        await vi.advanceTimersByTimeAsync(1);
        await second;
        expect(fn).toHaveBeenCalledTimes(2);
    });

    it("runs immediately when called after the window", async () => {
        const fn = vi.fn(async () => {});
        const run = throttled(fn, 1000);

        await run();
        vi.advanceTimersByTime(1000);
        await run();

        expect(fn).toHaveBeenCalledTimes(2);
    });

    it("propagates rejections from fn", async () => {
        const run = throttled(async () => { throw new Error("boom"); }, 1000);

        await expect(run()).rejects.toThrow("boom");
    });
});

describe("useTimeout", () => {
    it("fires the callback once after the delay", () => {
        const callback = vi.fn();
        renderHook(() => useTimeout(callback, 500));

        vi.advanceTimersByTime(499);
        expect(callback).not.toHaveBeenCalled();

        vi.advanceTimersByTime(1);
        expect(callback).toHaveBeenCalledTimes(1);

        vi.advanceTimersByTime(5000);
        expect(callback).toHaveBeenCalledTimes(1);
    });

    it("calls the latest callback without restarting the timer", () => {
        const first = vi.fn();
        const second = vi.fn();
        const { rerender } = renderHook(
            ({ callback }) => useTimeout(callback, 500),
            { initialProps: { callback: first } },
        );

        vi.advanceTimersByTime(300);
        rerender({ callback: second });

        vi.advanceTimersByTime(199);
        expect(second).not.toHaveBeenCalled();

        vi.advanceTimersByTime(1);
        expect(first).not.toHaveBeenCalled();
        expect(second).toHaveBeenCalledTimes(1);
    });

    it("restarts the timer when the delay changes", () => {
        const callback = vi.fn();
        const { rerender } = renderHook(
            ({ delay }) => useTimeout(callback, delay),
            { initialProps: { delay: 500 } },
        );

        vi.advanceTimersByTime(300);
        rerender({ delay: 1000 });

        vi.advanceTimersByTime(999);
        expect(callback).not.toHaveBeenCalled();

        vi.advanceTimersByTime(1);
        expect(callback).toHaveBeenCalledTimes(1);
    });

    it("does not fire after unmount", () => {
        const callback = vi.fn();
        const { unmount } = renderHook(() => useTimeout(callback, 500));

        unmount();
        vi.advanceTimersByTime(1000);

        expect(callback).not.toHaveBeenCalled();
    });

    it("does not schedule when delay is null", () => {
        const callback = vi.fn();
        // The signature says `number`, but the implementation guards on null.
        renderHook(() => useTimeout(callback, null as unknown as number));

        vi.advanceTimersByTime(10_000);

        expect(callback).not.toHaveBeenCalled();
    });
});
