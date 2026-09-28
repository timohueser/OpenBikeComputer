import { mount } from "svelte";
import "./styles/tokens.css";
import MapStudy from "./components/planner/MapStudy.svelte";

mount(MapStudy, { target: document.getElementById("app")! });
