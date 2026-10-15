import { useEffect, useState } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import "./App.css";

function App() {
  const [name, setName] = useState("besto_testo");
  const [server, setServer] = useState("warmitup.chat");
  const [port, setPort] = useState(64738);
  const [password, setPassword] = useState("");

  const [channel, setChannel] = useState<Channel<string> | null>(new Channel<string>());
  useEffect(() => {
    if (channel) {
      channel.onmessage = (event) => {
        console.log("Received message:", event);
      };
    }
  }, [channel]);

  return (
    <main className="container">
      <section className="user-list">
        User list
      </section>
      <section className="chat">
        Chat
      </section>

      {/* Form for connection */}
      <form className="connection-form"
        onSubmit={async (e) => {
          e.preventDefault();
          try {
            // Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
            await invoke("connect_disconnect", { server, port, password, name, channel });
          } catch (error) {
            console.error(error);
            alert("Failed to connect: " + error);
          }
        }}>
        <label>
          Server
          <input placeholder="Server" value={server} onChange={(e) => setServer(e.target.value)} />
        </label>
        <label>
          Port
          <input placeholder="Port" value={port} onChange={(e) => setPort(Number(e.target.value))} />
        </label>
        <label>
          Password
          <input placeholder="Password" type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
        </label>
        <label>
          Name
          <input placeholder="Name" value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <br />
        <button type="submit">
          Connect/Disconnect
        </button>
      </form>
    </main>
  );
}

export default App;
