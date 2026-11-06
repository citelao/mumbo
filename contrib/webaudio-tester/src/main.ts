const levelMeter = document.getElementById("level-meter") as HTMLProgressElement;
const levelText = document.getElementById("level-text") as HTMLElement;
const startStopButton = document.getElementById("start-stop") as HTMLButtonElement;
const echoCancellationInput = document.getElementById("echo-cancellation") as HTMLInputElement;
const noiseSuppressionInput = document.getElementById("noise-suppression") as HTMLInputElement;
const autoGainControlInput = document.getElementById("auto-gain-control") as HTMLInputElement;
const recordingsList = document.getElementById("recordings") as HTMLElement;

/** Quietest level shown on the meter. */
const METER_FLOOR_DB = -80;

let isRunning = false;
let stream: MediaStream | null = null;
let audioContext: AudioContext | null = null;
let recorder: MediaRecorder | null = null;
let recordingCount = 0;

startStopButton.addEventListener("click", () => {
    if (isRunning) {
        handleStop();
    } else {
        handleStart();
    }
});

/**
 * RMS level of the latest samples in dBFS (0 is full scale).
 *
 * Uses time-domain samples, not the analyser's frequency bins, so the number
 * means the same thing as the level of the recorded file.
 */
function getLevelDb(node: AnalyserNode, samples: Float32Array<ArrayBuffer>) {
    node.getFloatTimeDomainData(samples);
    let sumOfSquares = 0;
    for (const sample of samples) {
        sumOfSquares += sample * sample;
    }
    const rms = Math.sqrt(sumOfSquares / samples.length);
    return 20 * Math.log10(Math.max(rms, 1e-8));
}

function addRecording(blob: Blob, label: string, slug: string) {
    recordingCount += 1;
    const url = URL.createObjectURL(blob);

    const item = document.createElement("li");
    item.append(`#${recordingCount}: ${label} `);

    const audio = document.createElement("audio");
    audio.controls = true;
    audio.src = url;
    item.append(audio, " ");

    const download = document.createElement("a");
    download.href = url;
    // The container depends on the browser (Chrome: webm, Safari: mp4, Firefox: ogg).
    const extension = blob.type.includes("mp4") ? "m4a" : blob.type.includes("ogg") ? "ogg" : "webm";
    download.download = `mic-${recordingCount}-${slug}.${extension}`;
    download.textContent = "download";
    item.append(download);

    recordingsList.append(item);
}

async function handleStart() {
    const constraints = {
        echoCancellation: echoCancellationInput.checked,
        noiseSuppression: noiseSuppressionInput.checked,
        autoGainControl: autoGainControlInput.checked,
    };
    stream = await navigator.mediaDevices.getUserMedia({ audio: constraints });

    // What the browser actually applied, which can differ from what we asked for.
    const applied = stream.getAudioTracks()[0].getSettings();
    const label = `echoCancellation=${applied.echoCancellation} `
        + `noiseSuppression=${applied.noiseSuppression} `
        + `autoGainControl=${applied.autoGainControl}`;
    // Same settings, short enough for a filename: ec-on-ns-off-agc-on.
    const onOff = (value: boolean | undefined) => (value ? "on" : "off");
    const slug = `ec-${onOff(applied.echoCancellation)}`
        + `-ns-${onOff(applied.noiseSuppression)}`
        + `-agc-${onOff(applied.autoGainControl)}`;

    // Record the stream as the browser delivers it (after any echo cancellation).
    const chunks: Blob[] = [];
    const mediaRecorder = new MediaRecorder(stream);
    recorder = mediaRecorder;
    mediaRecorder.addEventListener("dataavailable", (event) => chunks.push(event.data));
    // Read the type from the local recorder: handleStop() clears `recorder`
    // before this event fires, and a Blob with no type won't play back.
    mediaRecorder.addEventListener("stop", () => {
        addRecording(new Blob(chunks, { type: mediaRecorder.mimeType }), label, slug);
    });
    recorder.start();

    // Not connected to the destination, so the mic is never played back live.
    audioContext = new AudioContext();
    const source = audioContext.createMediaStreamSource(stream);
    const analyser = audioContext.createAnalyser();
    source.connect(analyser);
    const samples = new Float32Array(analyser.fftSize);

    function updateLevelMeter() {
        const db = getLevelDb(analyser, samples);
        levelMeter.value = Math.max(0, (db - METER_FLOOR_DB) / -METER_FLOOR_DB);
        levelText.textContent = `${db.toFixed(1)} dBFS`;
        if (isRunning) {
            requestAnimationFrame(updateLevelMeter);
        }
    }

    isRunning = true;
    updateLevelMeter();
    startStopButton.textContent = "Stop";
}

function handleStop() {
    // Stop the recorder first so it flushes its last chunk before the tracks end.
    recorder?.stop();
    recorder = null;

    if (stream) {
        stream.getTracks().forEach(track => track.stop());
        stream = null;
    }

    void audioContext?.close();
    audioContext = null;

    isRunning = false;
    startStopButton.textContent = "Start";
}
