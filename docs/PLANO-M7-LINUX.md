# Telinhas — Plano do Linux (parte do M7)

Objetivo: o Linux faz **tudo** o que o Windows faz hoje: transmitir tela ou janela com som, assistir, atualizar-se. Segue DEC-29 e DEC-45: só **Wayland**, com `xdg-desktop-portal` e PipeWire. X11 fica fora (sessões X11 de GNOME e KDE podem funcionar de graça, mas não são promessa). Distribuição em **AppImage** (preferência do mantenedor; sem Flatpak).

## O que existe e o que falta

| Peça | Windows | Linux hoje |
|------|---------|------------|
| Interface, sala, rede, decodificação, reprodução de som | pronto | **pronto** (compila e usa ALSA pelo `cpal`) |
| Captura de vídeo | Windows Graphics Capture | falta |
| Captura de áudio (aplicativo, sistema, exclusões) | WASAPI process loopback | falta |
| Encode por hardware | NVENC (FFmpeg) | falta (NVENC compila igual; VAAPI para AMD e Intel não existe) |
| Encode por software | openh264 | pronto |
| Instalador e atualização | Inno Setup + atualizador | falta (AppImage) |

## Como cada peça vai ser feita

**Captura de vídeo.** O app pede a tela ao portal `org.freedesktop.portal.ScreenCast` (crate `ashpd`). O **portal mostra o seletor de tela ou janela do próprio sistema** (DEC-29), então o modal de transmissão no Linux troca as abas de telas e janelas por um botão "Escolher o que transmitir". O portal devolve um descritor do PipeWire, e os quadros chegam por um stream do PipeWire (crate `pipewire`), em memória compartilhada (BGRx, BGRA, RGBx, RGBA). O cursor pode ir dentro da imagem, conforme a opção "capturar o mouse". Entra como `Source::Portal` no crate `telinhas-capture`.

**Captura de áudio.** O PipeWire tem um nó por aplicativo que toca som. O desenho é o mesmo do Windows:
- *uma janela*: captura o nó do programa dono da janela e dos processos que ele criou (o PID vem do portal, ou da lista de nós);
- *tela inteira*: um stream por nó de aplicativo que não foi excluído, misturados, acompanhando quem começa e para de tocar, sempre sem o próprio Telinhas.
A mistura de 20 ms em 48 kHz estéreo e o codificador Opus são os que já existem.

**Encode.** O NVENC pelo FFmpeg reaproveita o código do Windows. **VAAPI** (AMD e Intel) precisa de superfícies de GPU e tem mais detalhe. Sem hardware, o openh264 por software serve, mas não sustenta 1080p60: o app já avisa quando cai no software (DEC-15). A ordem é NVENC e software primeiro, e VAAPI depois.

**AppImage.**
- Construído numa base antiga (Debian 12, glibc 2.36), para rodar em Debian, Ubuntu 22.04+, Fedora e Arch.
- Runtime estático (sem FUSE 2).
- Vão dentro: FFmpeg (LGPL) e as bibliotecas de apoio. Ficam no sistema: GPU, Vulkan, Wayland, PipeWire e ALSA/PulseAudio, que o `wgpu` e o `winit` carregam em tempo de execução.
- Atualização: o atualizador de hoje baixa o AppImage novo e troca o arquivo atual (o AppImage informa o caminho em `$APPIMAGE`), em vez de rodar um instalador.

## Ordem

1. **L1 — Linux compilando e rodando**, com a transmissão escondida até a captura existir. Testável aqui no WSLg.
2. **L2 — captura de vídeo** (portal + PipeWire) e o modal adaptado. Só testável numa sessão Wayland real.
3. **L3 — captura de áudio** por PipeWire. Idem.
4. **L4 — encode**: NVENC e software; VAAPI depois.
5. **L5 — AppImage e atualização.**
6. **L6 — prova** entre Windows e Linux, com medição.

## O que cada passo precisa de você

- **Bibliotecas de desenvolvimento no WSL** (para eu compilar a captura): `sudo apt install -y libpipewire-0.3-dev libspa-0.2-dev libclang-dev pkg-config`.
- **Uma máquina de teste com desktop Wayland de verdade** (L2 e L3). O WSLg não tem portal de captura nem PipeWire. Uma VM com **Fedora Workstation** (GNOME, Wayland, PipeWire e portais já vêm certos) é o caminho mais curto. A VM não terá GPU, então o encode será por software, e basta para provar a captura e o som. A prova de desempenho (60 fps) precisa de uma máquina Linux real, com GPU.
- **Testar o AppImage** em Debian 12, Fedora e Arch (L5), pelo WSL (`wsl --install Debian`, importar as outras) ou pela VM.

## Riscos

- Sem Linux real, não consigo medir fps nem latência no Linux; só funcionalidade.
- O portal pede confirmação a cada transmissão (pode permitir "lembrar", conforme o desktop).
- Cada desktop implementa o portal de um jeito (GNOME, KDE e wlroots). Vamos provar no GNOME primeiro (a VM), e KDE e Sway entram quando houver quem teste.
