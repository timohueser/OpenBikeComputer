import { mount } from "svelte";
import "leaflet/dist/leaflet.css";
import "./styles/app.css";
import App from "./App.svelte";

mount(App, { target: document.getElementById("app")! });
