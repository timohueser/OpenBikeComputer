import { mount } from "svelte";
import "./styles/tokens.css";
import "./styles/data.css";
import DataApp from "./components/data/DataApp.svelte";

mount(DataApp, { target: document.getElementById("app")! });
