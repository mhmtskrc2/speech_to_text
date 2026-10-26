MP3 adresinden (veya yerel ses dosyasından) metin üreten masaüstü uygulaması.
Rust kurmaya gerek yok — aşağıdaki paketi indirip çalıştırmanız yeterli.

| İndirme | Kimin için |
|---|---|
| `Sesten-Metne-GPU.exe` | Windows + ekran kartı (NVIDIA / AMD / Intel) — **hızlı** |
| `Sesten-Metne.exe` | Windows, ekran kartı sürücüsü olmayan makineler |
| `sesten-metne-macos` | Tüm Mac'ler — evrensel ikili, Metal hızlandırmalı |

Windows'ta iki dosya olmasının sebebi teknik bir zorunluluk: GPU sürümü
`vulkan-1.dll`'e yükleme zamanında bağlanır, o da ekran kartı sürücüsüyle
gelir. Sürücüsü olmayan bir makinede GPU sürümü hiç açılmaz, bu yüzden CPU
sürümü ayrı duruyor. **Önce GPU sürümünü deneyin**; açılmazsa CPU sürümünü
indirin. Uygulamanın "Gelişmiş" bölümündeki *Hızlandırma:* satırı hangi
aygıtın kullanıldığını adıyla yazar.

Yukarıdaki listede `Sesten-Metne-GPU.exe` yoksa o sürümde GPU derlemesi
üretilememiştir; CPU sürümü her durumda çalışır.

Mac dosyası evrensel (universal) bir ikilidir; hangi işlemciye sahip olduğunuzu
bilmenize gerek yok. Metal hızlandırması hazır gelir.

### Windows'ta ilk açılış

İndirdiğiniz exe'yi çalıştırın (önce **Sesten-Metne-GPU.exe**'yi deneyin).
SmartScreen uyarı verirse **Ek bilgi** → **Yine de çalıştır** deyin
(uygulama imzalı değil).

GPU sürümü "vulkan-1.dll bulunamadı" gibi bir hatayla açılmazsa ekran kartı
sürücünüz eksik demektir: ya sürücüyü güncelleyin ya da **Sesten-Metne.exe**
(CPU sürümü) indirin.

Açılmazsa [Microsoft Visual C++ Redistributable](https://aka.ms/vs/17/release/vc_redist.x64.exe)
kurulu olmayabilir; kurup tekrar deneyin.

### macOS'ta ilk açılış

İndirilen dosya tek bir çalıştırılabilir ikilidir. Tarayıcı indirmede
çalıştırma iznini düşürdüğü ve dosya imzalı olmadığı için, Terminal'de bir kez
şunları çalıştırın:

```bash
chmod +x ~/Downloads/sesten-metne-macos
xattr -d com.apple.quarantine ~/Downloads/sesten-metne-macos
```

Sonra dosyaya çift tıklayarak ya da Terminal'den çalıştırabilirsiniz.

### Hız hakkında

Model seçimi süreyi doğrudan belirler. Apple Silicon bir Mac'te ölçülen,
**1 saatlik ses** için yaklaşık süreler:

| Model | Apple Silicon (Metal) | Yalnızca CPU |
|---|---|---|
| `base` | ~2 dakika | ~7 dakika |
| `large-v3-turbo (q5_0)` | ~5 dakika | ~26 dakika |
| `large-v3` | ~9 dakika | ~44 dakika |

(63 saniyelik Türkçe bir kayıtla ölçülüp bir saate oranlandı; model zaten
indirilmiş kabul edilmiştir. Kendi makineniz ve ses kalitenize göre değişir.)

Mac dosyasında Metal hazır gelir. Windows'ta GPU sürümünü indirdiyseniz sol
sütuna yakın, CPU sürümünü indirdiyseniz sağ sütun geçerlidir (Vulkan, Metal
kadar hızlı olmayabilir). `large-v3` en doğru sonucu verir ama en yavaşıdır; günlük
kullanımda `large-v3-turbo (q5_0)` doğruluk/hız dengesi olarak çok daha iyidir.

İlerleme çubuğunun yanında geçen süre ve tahmini kalan süre gösterilir, yani
yavaş bir işi askıda kalmış bir işten ayırt edebilirsiniz.

### İlk çalıştırmada ne olur

Seçtiğiniz konuşma tanıma modeli bir kez indirilir ve önbelleğe alınır;
sonraki çalıştırmalar internet gerektirmez. Varsayılan `large-v3` en doğru
sonucu verir ama **2.9 GB** indirir — hemen başlamak için menüden daha küçük
bir model (örneğin `base`, 141 MB) seçebilirsiniz.

Ses dosyanız hiçbir sunucuya gönderilmez, her şey bilgisayarınızda işlenir.
