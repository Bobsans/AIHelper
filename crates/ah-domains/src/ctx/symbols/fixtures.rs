//! One fixture per dispatch arm of `extract_symbols`, covering every extraction
//! pattern the table defines.
//!
//! The corpus is checked mechanically: each regex in the table matches at least
//! one line here. That matters because the module had no unit tests of its own
//! and the integration tests assert only that a handful of expected symbols are
//! *present*, never that the whole extraction is unchanged.
//!
//! Behind a feature rather than `cfg(test)`: the golden snapshot that renders it
//! lives in the CLI crate, and one crate's test configuration is invisible to
//! another's. A release build enables no dev-dependencies, so it compiles none
//! of this.

#[cfg(any(test, feature = "fixtures"))]
pub const SYMBOL_FIXTURES: &[(&str, &str)] = &[
    (
        "lib.rs",
        "pub async fn build_index(root: &Path) -> Result<()> {\nstruct Config {\npub enum Mode {\ntrait Render {\nimpl<T: Clone> Render for Config {\npub mod helpers {\n",
    ),
    (
        "README.md",
        "# Title\n## Section\n### Sub section\n#### Deep\n",
    ),
    (
        "app.py",
        "class Service:\n    async def handle(self):\n    def plain(self):\n",
    ),
    (
        "app.ts",
        "export class Widget {\nexport async function render() {\nexport interface Props {\nexport type Alias = string;\nexport const build = async (\n",
    ),
    ("main.go", "func Serve() {\ntype Server struct {\n"),
    (
        "Main.java",
        "package com.example.demo;\npublic sealed interface Service {\npublic class App {\n    public static String render(int value) {\n",
    ),
    (
        "App.kt",
        "package com.example.demo\ndata class Person(val name: String)\nprivate fun boot() {\n",
    ),
    (
        "App.scala",
        "package com.example\ncase class Item(name: String)\noverride def run(): Unit = {\n",
    ),
    (
        "Program.cs",
        "namespace Demo.App;\npublic record Item(string Name);\npublic static string Render(int value) {\n",
    ),
    (
        "lib.php",
        "namespace App\\Domain;\nabstract class Handler {\nfunction handle() {\n",
    ),
    ("worker.rb", "module Demo\nclass Worker\n  def perform!\n"),
    (
        "app.ex",
        "defmodule Demo.Worker do\n  def perform do\n  defp helper? do\n  defmacro guarded do\n",
    ),
    ("app.erl", "-module(demo_worker).\nhandle(Request) ->\n"),
    (
        "App.swift",
        "public struct Model {\ninternal actor Store {\nprivate func reload() {\n",
    ),
    (
        "main.dart",
        "class Widget {\nmixin Logging {\nvoid render(\n",
    ),
    (
        "main.cpp",
        "namespace demo::core {\nclass Engine {\nstruct Point {\nint compute(int value) {\n",
    ),
    (
        "main.zig",
        "pub fn main() void {\nconst Config = struct {\n",
    ),
    (
        "init.lua",
        "local function setup()\nfunction M.teardown()\n",
    ),
    ("Module.pm", "package Demo::Module;\nsub render {\n"),
    (
        "analysis.r",
        "summarise <- function(data) {\nplot.data = function(x) {\n",
    ),
    (
        "model.jl",
        "struct Point\nmutable struct Buffer\nfunction solve(x)\n",
    ),
    (
        "Lib.hs",
        "module Demo.Lib where\ndata Shape = Circle\nnewtype Wrapper = Wrapper Int\nclass Render a where\nrender :: Shape -> String\n",
    ),
    (
        "lib.ml",
        "module Store = struct\ntype shape = Circle\nlet rec walk node =\n",
    ),
    (
        "main.tf",
        "resource \"aws_s3_bucket\" \"logs\" {\nmodule \"network\" {\nvariable \"region\" {\n",
    ),
    ("config.yml", "service:\nimage_name:\n"),
    ("Cargo.toml", "[package]\n[[bin]]\n[dependencies.serde]\n"),
    ("deploy.sh", "function build() {\nteardown() {\n"),
    ("Module.psm1", "function Invoke-Build {\n"),
    (
        "Dockerfile",
        "FROM rust:1.88 AS builder\nFROM debian:bookworm\n",
    ),
    ("Makefile", "build:\ntest-all:\n"),
    (
        "notes.txt",
        "class Loose\ninterface Loose\nfn loose\ndef loose\n",
    ),
];
