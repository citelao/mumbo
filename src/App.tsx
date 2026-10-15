import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

function App() {
  const [greetMsg, setGreetMsg] = useState("");
  const [name, setName] = useState("besto_testo");
  const [server, setServer] = useState("warmitup.chat");
  const [port, setPort] = useState(64738);
  const [password, setPassword] = useState("");

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
      <form className="connection-form">
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
        <button type="submit" onClick={async () => {
          try {
            await invoke("connect_disconnect", { server, port, password, name });
          } catch (error) {
            console.error(error);
            alert("Failed to connect: " + error);
          }
        }}>
          Connect/Disconnect
        </button>
      </form>
    </main>
  );
}

export default App;
