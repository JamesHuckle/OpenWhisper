import { Nav } from "@/components/nav";
import { Hero } from "@/components/hero";
import { DemoSection } from "@/components/demo-section";
import { HowItWorks } from "@/components/how-it-works";
import { Features } from "@/components/features";
import { CTASection } from "@/components/cta-section";
import { Footer } from "@/components/footer";

export default function Home() {
  return (
    <>
      <Nav />
      <main>
        <Hero />
        <DemoSection />
        <HowItWorks />
        <Features />
        <CTASection />
      </main>
      <Footer />
    </>
  );
}
