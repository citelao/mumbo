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

async function handleStart() {
    stream = await navigator.mediaDevices.getUserMedia({ audio: true });

    const audioContext = new AudioContext();
    const source = audioContext.createMediaStreamSource(stream);
    const analyser = audioContext.createAnalyser();
    source.connect(analyser);

    function updateLevelMeter() {
        const data = new Uint8Array(analyser.frequencyBinCount);
        analyser.getByteFrequencyData(data);
        const level = data.reduce((a, b) => a + b, 0) / data.length;

        console.log(level, data);
        levelMeter.value = level;
        if (isRunning) {
            requestAnimationFrame(updateLevelMeter);
        }
    }
    updateLevelMeter();
    
    isRunning = true;
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