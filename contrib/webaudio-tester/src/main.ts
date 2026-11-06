const levelMeter = document.getElementById("level-meter") as HTMLProgressElement;
const startStopButton = document.getElementById("start-stop") as HTMLButtonElement;
const audioInputSelect = document.getElementById("audio-input") as HTMLSelectElement;
const audioOutputSelect = document.getElementById("audio-output") as HTMLSelectElement;

let isRunning = false;
let stream: MediaStream | null = null;

startStopButton.addEventListener("click", () => {
    if (isRunning) {
        handleStop();
    } else {
        handleStart();
    }
});

/** Gets RMS level from [0-256) */
function getRmsLevel(node: AnalyserNode) {
    const data = new Uint8Array(node.frequencyBinCount);
    node.getByteFrequencyData(data);
    const rms = Math.sqrt(data.reduce((a, b) => a + (b * b), 0) / data.length);
    console.log(rms, data);

    return rms;
}

async function handleStart() {
    stream = await navigator.mediaDevices.getUserMedia({ audio: true });

    const audioContext = new AudioContext();
    const source = audioContext.createMediaStreamSource(stream);
    const analyser = audioContext.createAnalyser();
    source.connect(analyser);

    function updateLevelMeter() {
        const level = getRmsLevel(analyser) / 256;
        // levelMeter.value = Math.random();
        levelMeter.value = level;
        if (isRunning) {
            requestAnimationFrame(updateLevelMeter);
        }
    }

    isRunning = true;
    updateLevelMeter();
    startStopButton.textContent = "Stop";
}

function handleStop() {
    if (stream) {
        stream.getTracks().forEach(track => track.stop());
        stream = null;
    }

    isRunning = false;
    startStopButton.textContent = "Start";
}