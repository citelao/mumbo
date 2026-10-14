import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

function App() {
  const [greetMsg, setGreetMsg] = useState("");
  const [name, setName] = useState("");
  const [server, setServer] = useState("warmitup.chat");
  const [port, setPort] = useState(64738);

  async function greet() {
    // Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
    setGreetMsg(await invoke("greet", { name }));
  }

  return (
    <main className="container">
      <section className="user-list">
        User list
      </section>
      <section className="chat">
        Chat
      </section>

      {/* Form for connection */}
      <label>
        Server
        <input placeholder="Server" value={server} onChange={(e) => setServer(e.target.value)} />
      </label>
      <label>
        Port
        <input placeholder="Port" value={port} onChange={(e) => setPort(Number(e.target.value))} />
      </label>
      <button onClick={async () => {
        try {
          await invoke("connect_disconnect", { server, port });
        } catch (error) {
          console.error(error);
        }
      }}>
        Connect/Disconnect
      </button>
    </main>
  );
}

export default App;
