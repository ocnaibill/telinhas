# Telinhas — Escolha da biblioteca de UI

> Decidido: egui com eframe e wgpu (DEC-61), com o spike M4a antes de fixar de vez. Parte das informações sobre as bibliotecas vem de pesquisa na web e está marcada como **a verificar** onde não pude confirmar no código.

## 1. O que a interface precisa fazer

Extraído das três telas de `docs/design/stitch/` (com o que a DEC-31 e o `design/README.md` mandam tirar) e das DECs:

| Necessidade | De onde vem |
|-------------|-------------|
| **Vários vídeos decodificados exibidos como texturas de GPU**, com sobreposições (rótulo "AO VIVO", fps, nome, volume, webcam em picture-in-picture dentro do cartão), a 60 fps | Sala ao vivo; DEC-12, DEC-33, DEC-40 |
| Layouts Grid dinâmico e Foco, painel lateral de transmissões com controle de volume, barra flutuante de controles | Sala ao vivo; DEC-33, DEC-34 |
| **Miniaturas ao vivo** de monitores e janelas no modal de configuração | Configurar transmissão; DEC-36 |
| Modal sobre a sala, abas, controles segmentados, interruptores, pop-ups e avisos (aprovação de entrada, reconectando, encode por software) | DEC-20, DEC-36, RF-19, RF-21 |
| Campos de texto (código da sala, apelido) | RF-01, RF-02, RF-15 |
| Tema escuro, cartões arredondados, índigo de ação, verde de "ao vivo", degradês leves | DEC-30 |
| Janela única que muda de tamanho | DEC-20 |
| Rust nativo, sem webview; Windows, macOS e Linux (Wayland) | DEC-01, DEC-28, DEC-29, DEC-45 |
| Licença compatível com um projeto Apache-2.0 | DEC-51 |

O vídeo é o centro do app. Decodificamos H.264 (hoje por software, depois por hardware), então o quadro chega como planos YUV ou NV12, e precisa virar RGB na GPU.

## 2. Candidatas

| | **egui** (com `eframe` e wgpu) | **iced** | **Slint** |
|---|---|---|---|
| Versão vista | 0.37.0 | 0.14.0 | 1.19.0 |
| Licença | MIT ou Apache-2.0 | MIT | **GPL-3.0, ou "Royalty-free" proprietária, ou comercial** |
| Modelo | Modo imediato | Elm (retido) | Declarativo, linguagem própria `.slint` |
| Vídeo como textura de GPU | `egui_wgpu::CallbackTrait` dá acesso ao `RenderPass` do wgpu, para desenhar um quadro YUV com shader próprio dentro de qualquer retângulo. Também há `register_native_texture`, que exige RGBA | Widget `shader` (pipeline wgpu próprio). Existe o crate `iced_video_player`, que desenha vídeo por wgpu, mas é baseado em GStreamer e a última versão publicada que vi é de 2024 | Importa `wgpu::Texture` com `Image::try_from`, mas a API é **instável** (atrelada à versão do wgpu, por feature `unstable-wgpu-NN`) e exige o mesmo dispositivo wgpu da Slint |
| Fidelidade ao visual do mockup | Média: cartões arredondados e cores são fáceis, degradês, sombras suaves e animações exigem pintura própria | Boa: degradês, sombras e temas nativos | Muito boa, a mais próxima do mockup, com pré-visualização ao vivo do `.slint` |
| Estabilidade da API | Quebra a cada versão (0.37 agora) | Quebra a cada versão | API 1.x estável, mas o trecho de wgpu é instável |
| Acessibilidade e IME | AccessKit integrado | Mais fraca (a verificar) | AccessKit |
| Testes de interface sem janela | `egui_kittest` (a verificar o estado atual) | a verificar | Backend de testes e renderizador por software |
| Maturidade e comunidade | A maior (cerca de dez vezes os downloads das outras, segundo um comparativo de 2026) | Média; usada no COSMIC da System76 | Média; foco forte em embarcados e design |

Descartadas por maturidade ou encaixe: GPUI (Zed, Apache-2.0, 0.2.2), Floem (0.2.0), Xilem (0.4.0, experimental) e Dioxus nativo.

## 3. Pontos que pesam mais

1. **Licença da Slint.** Usar a Slint sob a GPL-3.0 obrigaria o binário distribuído a ser GPL-3.0, o que conflita com a DEC-51 (Apache-2.0). A licença "Royalty-free" evita isso, mas é proprietária (exige atribuição à Slint) e todo fork do Telinhas teria de aceitá-la também. Para um projeto aberto Apache-2.0, isso é um custo real, e por si só já a deixa para trás.
2. **Caminho do vídeo.** É o requisito principal. No egui é o caminho mais documentado e usa o wgpu que de qualquer forma precisamos para NV12 e YUV. No iced é viável (widget `shader`). Na Slint depende de uma API instável.
3. **Fidelidade ao design.** É onde o egui perde. A mitigação é um módulo de tema próprio e pintura personalizada para os poucos elementos que o mockup pede (cartão com borda de cor, selo "AO VIVO", barra flutuante). O vídeo e as sobreposições são, por natureza, pintura própria em qualquer biblioteca.
4. **Quebras entre versões.** egui e iced mudam a API a cada versão. A mitigação é fixar versão e isolar a interface atrás de uma camada fina de visão.

## 4. Recomendação

**egui, com `eframe` e o backend wgpu**, para o M4.

- O caminho do vídeo é o mais direto e o menos arriscado.
- A licença é compatível com a DEC-51.
- É a maior comunidade, com o iterar mais rápido para os muitos fluxos do M4 (sala, aprovação, reconexão, modal).
- O WSL deste ambiente tem WSLg (Wayland e X11), então a janela do egui aparece na sua área de trabalho do Windows durante o desenvolvimento, e eu posso rodá-la daqui.
- O custo conhecido é o acabamento visual, que fica em segundo plano no MVP e pode ser refinado depois.

**Segunda opção: iced**, se o visual do mockup virar prioridade antes do desempenho de vídeo ou se o egui mostrar um limite sério no spike abaixo. **Slint: não recomendada**, pela licença.

## 5. Antes de fixar: um spike curto (M4a)

A hipótese de risco é "o egui mostra 3 ou mais streams de 1080p60 por shader próprio, a 60 fps, com baixo uso de CPU". Proponho validar com um spike antes de gastar o M4 inteiro:

- uma janela `eframe` com wgpu mostrando N cartões de vídeo alimentados pelo nosso receptor (ou por vídeo sintético em 1080p), com shader YUV→RGB, selo "AO VIVO" e uma sobreposição de webcam;
- medir fps da interface, CPU, GPU e latência de apresentação com 1, 3 e 6 cartões;
- repetir o mesmo, em menor escala, no iced se o egui falhar.

Se o spike passar, o M4 segue com o egui. Se falhar, eu volto com os números e a troca para o iced.

## 6. Decisões que dependem de você

- Aceita a recomendação (egui), com o spike M4a antes de fixar?
- Aceita descartar a Slint pela licença, ou quer manter a opção "Royalty-free" aberta?

## 7. Fontes

- Versões e licenças: `cargo info` de cada crate, consultado em 17/11/2026.
- Comparativos gerais (2026): <https://www.pistack.xyz/posts/2026-08-25-rust-gui-frameworks-egui-iced-slint-comparison/> e <https://www.libhunt.com/compare-iced-vs-slint>
- Licença da Slint: <https://slint.dev/pricing> e <https://cdn.jsdelivr.net/gh/slint-ui/slint@1.16.0/LICENSE.md>
- Importação de textura wgpu na Slint: <https://docs.slint.dev/latest/docs/rust/slint/wgpu_28> e <https://git.joshthomas.dev/language-servers/slint/src/branch/master/examples/wgpu_texture/README.md>
- `iced_video_player`: <https://github.com/jazzfool/iced_video_player> e <https://lib.rs/crates/iced_video_player>
- Gancho de pintura personalizada do egui: <https://docs.rs/egui-wgpu/latest/egui_wgpu/trait.CallbackTrait.html>

## 8. Spike M4a: estado

`ui-spike/` (`telinhas-ui-spike`): janela `eframe` com wgpu, N cartões de vídeo (grade dinâmica) alimentados por quadros I420 em 1080p60 gerados por threads, cada um enviado à GPU como três texturas (`R8Unorm`) e convertido para RGB por um shader dentro de um `CallbackTrait` do `egui_wgpu`, com cantos arredondados no próprio shader. Sobre o vídeo: selo "AO VIVO", plaqueta com avatar e nome, borda verde, webcam em picture-in-picture, barra flutuante de controles e painel lateral com volume, como na tela "sala ao vivo". Mede fps da interface, tempo por quadro (p50, p95, p99, máximo), CPU e MB/s enviados à GPU.

**Compilou na primeira com o egui 0.37 e o wgpu 30** e rodou no WSLg (software, `lavapipe`), o que confirma que o caminho de pintura personalizada funciona. Os números do WSLg **não valem**: sem GPU de verdade a rasterização é por software (CPU em 400%). A medição que importa é no Windows, com a GPU.

Opções: `--tiles N`, `--width`, `--height`, `--fps`, `--seconds`, `--static-frames` (isola o custo da interface do custo do vídeo), `--no-vsync`, `--no-webcam`.
